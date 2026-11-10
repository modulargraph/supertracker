//! The per-instance proxy: shared state and the real-time `process` call.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use stpc_transport::monotonic_ns;
use stpc_transport::region::{
    next_seq, Event, Region, Transport, REQ_DISCONTINUITY, REQ_OFFLINE, RSP_BUSY, RSP_ERROR, RSP_INACTIVE,
    RSP_OUT_EVENTS_DROPPED, RSP_PROCESSED, SEQ_ABORT,
};
use stpc_transport::wake::{Notifier, WaitOutcome, Waiter};
use stpc_transport::MAX_CHANNELS;

use crate::supervisor::{Control, SupInner};
use crate::InstanceId;

/// The plug-in processed the block; the outputs are its audio.
pub const PROCESS_OK: u32 = 1;
/// The outputs hold the fallback (silence or dry signal), for one of the reasons below.
pub const PROCESS_FALLBACK: u32 = 2;
/// The container missed the deadline (now, or for an earlier block it has not finished).
pub const PROCESS_LATE: u32 = 4;
/// The container is gone (crashed, killed, hung and given up, or shut down).
pub const PROCESS_LOST: u32 = 8;
/// A control operation holds the instance on either side (activation, reconfiguration,
/// restore); try again next block.
pub const PROCESS_BUSY: u32 = 16;
/// The instance is not activated.
pub const PROCESS_INACTIVE: u32 = 32;
/// Bypassed by the client (the plug-in has no bypass of its own).
pub const PROCESS_BYPASSED: u32 = 64;
/// First processed block after dropped ones; the plug-in was told about the discontinuity.
pub const PROCESS_RESYNCED: u32 = 128;
/// More events were passed than the instance's event capacity; the excess was dropped.
pub const PROCESS_EVENTS_DROPPED: u32 = 256;
/// The plug-in produced more events than fit (its list or the caller's buffer).
pub const PROCESS_OUT_EVENTS_DROPPED: u32 = 512;
/// The plug-in reported a processing error; outputs are silent.
pub const PROCESS_PLUGIN_ERROR: u32 = 1024;
/// Bad arguments (more frames than `max_frames`, an output slice shorter than `frames`).
pub const PROCESS_INVALID: u32 = 2048;

/// `RawBlock::deadline_ns` value for "wait as long as it takes".
pub const DEADLINE_NONE: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum InstanceStatus {
    Running = 0,
    /// A block missed its deadline and the container has not caught up yet. `process`
    /// returns the fallback without waiting; it resumes by itself once the container is done.
    Late = 1,
    /// The container is gone. `process` returns the fallback at once until `restore`.
    Lost = 2,
    /// Destroyed, or the supervisor was shut down.
    Closed = 3,
}

impl InstanceStatus {
    pub(crate) fn from_u32(value: u32) -> InstanceStatus {
        match value {
            0 => InstanceStatus::Running,
            1 => InstanceStatus::Late,
            2 => InstanceStatus::Lost,
            _ => InstanceStatus::Closed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginKind {
    Effect,
    Instrument,
}

/// What `process` writes to the outputs when the plug-in cannot be reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Silence for instruments, pass-through for effects.
    Auto,
    Silence,
    /// Main input bus copied to the main output bus (delayed by the plug-in's reported
    /// latency if `max_dry_delay_frames` allows), other outputs silent.
    PassThrough,
}

#[derive(Clone, Debug)]
pub struct InstanceConfig {
    /// Backend name in the container: "clap", "vst3", "au", "test".
    pub format: String,
    /// Plug-in identifier, in the backend's terms.
    pub plugin_id: String,
    /// Groups instances under [`crate::GroupingPolicy::PerManufacturer`].
    pub manufacturer: String,
    /// Display name, for logs and container process titles.
    pub name: String,
    /// Backend-specific options (e.g. the bundle path).
    pub options: String,
    pub kind: PluginKind,
    pub sample_rate: f64,
    /// Largest block `process` will be called with.
    pub max_frames: u32,
    /// Channels per input bus. Bus 0 is the main input; further buses are sidechains.
    pub input_buses: Vec<u32>,
    pub output_buses: Vec<u32>,
    /// Most events per block, host -> plug-in.
    pub event_capacity: u32,
    /// Most events per block, plug-in -> host (MIDI out).
    pub out_event_capacity: u32,
    /// Capacity of the plug-in -> host notification queue (a power of two).
    pub notification_capacity: u32,
    /// How long `process` waits for the container. `None`: the duration of the block
    /// (`frames / sample_rate`), but at least 2 ms. Ignored in offline mode.
    pub deadline: Option<Duration>,
    pub fallback: Fallback,
    /// Largest plug-in latency the dry fallback can be delayed by, so that a failing or
    /// bypassed effect stays aligned with the host's delay compensation. 0 disables the
    /// delay line (the fallback is then the undelayed input).
    pub max_dry_delay_frames: u32,
    /// State to load right after creation.
    pub initial_state: Option<Vec<u8>>,
}

impl InstanceConfig {
    /// A stereo effect at 48 kHz, blocks up to 4096 frames.
    pub fn effect(format: &str, plugin_id: &str) -> InstanceConfig {
        InstanceConfig {
            format: format.into(),
            plugin_id: plugin_id.into(),
            manufacturer: String::new(),
            name: plugin_id.into(),
            options: String::new(),
            kind: PluginKind::Effect,
            sample_rate: 48_000.0,
            max_frames: 4096,
            input_buses: vec![2],
            output_buses: vec![2],
            event_capacity: 1024,
            out_event_capacity: 256,
            notification_capacity: 1024,
            deadline: None,
            fallback: Fallback::Auto,
            max_dry_delay_frames: 0,
            initial_state: None,
        }
    }

    /// A stereo instrument (no audio input) at 48 kHz.
    pub fn instrument(format: &str, plugin_id: &str) -> InstanceConfig {
        InstanceConfig { kind: PluginKind::Instrument, input_buses: Vec::new(), ..InstanceConfig::effect(format, plugin_id) }
    }
}

/// One block for [`Instance::process_raw`]: raw channel pointers, as audio APIs deliver them.
#[derive(Clone, Copy, Debug)]
pub struct RawBlock {
    pub frames: u32,
    /// `num_inputs` channel pointers (all buses, in order); a null channel is silent.
    pub inputs: *const *const f32,
    pub num_inputs: u32,
    /// `num_outputs` channel pointers; a null channel is skipped.
    pub outputs: *const *mut f32,
    pub num_outputs: u32,
    /// Events sorted by offset.
    pub events: *const Event,
    pub num_events: u32,
    /// Room for events from the plug-in (may be null with capacity 0).
    pub out_events: *mut Event,
    pub out_event_capacity: u32,
    /// Null: stopped transport without tempo.
    pub transport: *const Transport,
    /// 0: the instance's configured deadline. [`DEADLINE_NONE`]: no limit.
    pub deadline_ns: u64,
}

impl Default for RawBlock {
    fn default() -> RawBlock {
        RawBlock {
            frames: 0,
            inputs: std::ptr::null(),
            num_inputs: 0,
            outputs: std::ptr::null(),
            num_outputs: 0,
            events: std::ptr::null(),
            num_events: 0,
            out_events: std::ptr::null_mut(),
            out_event_capacity: 0,
            transport: std::ptr::null(),
            deadline_ns: 0,
        }
    }
}

/// One block for [`Instance::process`].
pub struct ProcessBuffers<'a, 'b> {
    pub frames: usize,
    /// Input channels of all buses, in order. A missing or too short channel is silent.
    pub inputs: &'a [&'b [f32]],
    /// Output channels of all buses, in order; each at least `frames` long.
    pub outputs: &'a mut [&'b mut [f32]],
    pub events: &'a [Event],
    /// Receives the plug-in's events (MIDI out); see [`ProcessOutcome::out_events`].
    pub out_events: &'a mut [Event],
    pub transport: Option<&'a Transport>,
    /// Overrides the instance's deadline for this block (e.g. the time left in the cycle).
    pub deadline: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessOutcome {
    /// `PROCESS_*` bits.
    pub flags: u32,
    /// Number of events written to the out-event buffer.
    pub out_events: usize,
    /// Bit n set: output channel n is known to be all zeros.
    pub output_silence: u64,
    /// Time the plug-in spent in its own process call (0 for fallback blocks).
    pub plugin_ns: u64,
}

impl ProcessOutcome {
    /// The plug-in's audio is in the outputs.
    pub fn processed(&self) -> bool {
        self.flags & PROCESS_OK != 0
    }
    /// The outputs hold the fallback.
    pub fn fell_back(&self) -> bool {
        self.flags & PROCESS_FALLBACK != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstanceStats {
    pub blocks: u64,
    /// Blocks answered with the fallback, for any reason.
    pub fallback_blocks: u64,
    /// Deadline misses (each starts a "late" episode).
    pub deadline_misses: u64,
    /// Episodes that ended with the container catching up.
    pub resyncs: u64,
    /// Notifications the plug-in could not queue (queue full).
    pub dropped_notifications: u64,
}

pub(crate) const GATE_IDLE: u32 = 0;
pub(crate) const GATE_PROCESSING: u32 = 1;
pub(crate) const GATE_EXCLUSIVE: u32 = 2;

/// The live attachment of an instance to a container.
pub(crate) struct Link {
    pub region: Arc<Region>,
    /// Wakes the container's real-time thread (request published).
    pub notifier: Notifier,
    /// Sleeps until the container publishes the response.
    pub waiter: Waiter,
}

/// Delay line over the main input, so the dry fallback can match the plug-in's latency.
pub(crate) struct DryDelay {
    data: Vec<f32>,
    channels: usize,
    capacity: usize,
    pos: usize,
    max_delay: usize,
}

impl DryDelay {
    pub fn new(channels: usize, max_delay: usize, max_frames: usize) -> DryDelay {
        let capacity = if max_delay == 0 || channels == 0 { 0 } else { max_delay + max_frames };
        DryDelay { data: vec![0.0; channels * capacity], channels, capacity, pos: 0, max_delay }
    }

    fn enabled(&self) -> bool {
        self.capacity != 0
    }

    /// # Safety
    /// `inputs` holds `num_inputs` pointers, each null or valid for `frames` samples;
    /// `frames <= max_frames`.
    unsafe fn feed(&mut self, inputs: *const *const f32, num_inputs: usize, frames: usize) {
        let first = frames.min(self.capacity - self.pos);
        for channel in 0..self.channels {
            let line = self.data.as_mut_ptr().add(channel * self.capacity);
            let src = if channel < num_inputs { *inputs.add(channel) } else { std::ptr::null() };
            if src.is_null() {
                std::ptr::write_bytes(line.add(self.pos), 0, first);
                std::ptr::write_bytes(line, 0, frames - first);
            } else {
                std::ptr::copy_nonoverlapping(src, line.add(self.pos), first);
                std::ptr::copy_nonoverlapping(src.add(first), line, frames - first);
            }
        }
        self.pos = (self.pos + frames) % self.capacity;
    }

    /// Copies the block that was fed last, delayed by `delay` frames, to `dst`.
    ///
    /// # Safety
    /// `dst` is valid for `frames` samples; `channel < channels`; `frames <= max_frames`.
    unsafe fn read(&self, channel: usize, delay: usize, frames: usize, dst: *mut f32) {
        let delay = delay.min(self.max_delay);
        let start = (self.pos + 2 * self.capacity - frames - delay) % self.capacity;
        let first = frames.min(self.capacity - start);
        let line = self.data.as_ptr().add(channel * self.capacity);
        std::ptr::copy_nonoverlapping(line.add(start), dst, first);
        std::ptr::copy_nonoverlapping(line, dst.add(first), frames - first);
    }
}

/// State owned by whoever holds the gate (normally the audio thread).
pub(crate) struct RtState {
    pub link: Option<Link>,
    seq: u32,
    /// Block number the container still owes after a deadline miss (0: none).
    pending: u32,
    /// Pipe mode: the container closed its end of the wake pipe (it is dead or dying).
    peer_gone: bool,
    discontinuity: bool,
    pub dry: DryDelay,
    pub max_frames: usize,
    event_capacity: usize,
    out_event_capacity: usize,
}

impl RtState {
    pub fn new() -> RtState {
        RtState {
            link: None,
            seq: 0,
            pending: 0,
            peer_gone: false,
            discontinuity: false,
            dry: DryDelay::new(0, 0, 0),
            max_frames: 0,
            event_capacity: 0,
            out_event_capacity: 0,
        }
    }

    /// Installs a fresh attachment (new region, block numbering restarts).
    pub fn install(&mut self, link: Link, dry: DryDelay) {
        let layout = link.region.layout();
        self.max_frames = layout.max_frames;
        self.event_capacity = layout.event_capacity;
        self.out_event_capacity = layout.out_event_capacity;
        self.link = Some(link);
        self.seq = 0;
        self.pending = 0;
        self.peer_gone = false;
        self.discontinuity = false;
        self.dry = dry;
    }
}

pub(crate) struct InstanceShared {
    pub id: InstanceId,
    /// Excludes `process` from control operations that replace the attachment.
    pub gate: AtomicU32,
    pub status: AtomicU32,
    pub active: AtomicBool,
    pub client_bypass: AtomicBool,
    pub offline: AtomicBool,
    pub latency: AtomicU32,
    pub tail: AtomicU64,
    /// Container generation the instance currently lives in.
    pub generation: AtomicU64,
    /// When the current late episode began (0: not late), and the block number owed.
    pub late_since_ns: AtomicU64,
    pub late_seq: AtomicU32,
    /// When the audio thread started waiting for the current block (0: not waiting).
    pub wait_started_ns: AtomicU64,
    pub blocks: AtomicU64,
    pub fallback_blocks: AtomicU64,
    pub deadline_misses: AtomicU64,
    pub resyncs: AtomicU64,
    /// Late episodes already reported as events (housekeeping thread).
    pub reported_misses: AtomicU64,
    pub hang_reported: AtomicBool,
    /// Configured deadline in ns (0: automatic), spin time before sleeping, sample rate.
    pub deadline_ns: AtomicU64,
    pub spin_ns: u64,
    pub sample_rate_bits: AtomicU64,
    pub pass_through: bool,
    pub main_in: usize,
    pub main_out: usize,
    pub total_in: usize,
    pub total_out: usize,
    pub rt: UnsafeCell<RtState>,
    /// The current region, for the housekeeping thread (notifications) and for releasing
    /// a waiting audio thread when the container dies. Held only briefly.
    pub region_ref: Mutex<Option<Arc<Region>>>,
    /// Parameter values set since the last state snapshot (replayed on restore).
    pub dirty: Mutex<HashMap<u64, f64>>,
    pub control: Mutex<Control>,
}

// SAFETY: `rt` is only accessed by the holder of `gate`; everything else is Sync.
unsafe impl Sync for InstanceShared {}
unsafe impl Send for InstanceShared {}

impl InstanceShared {
    pub fn status(&self) -> InstanceStatus {
        InstanceStatus::from_u32(self.status.load(Ordering::Acquire))
    }

    /// Blocks until the audio thread is outside `process`, then keeps it out (it gets
    /// `PROCESS_BUSY` fallbacks) until [`InstanceShared::unlock_gate`].
    pub fn lock_gate(&self) {
        while self
            .gate
            .compare_exchange(GATE_IDLE, GATE_EXCLUSIVE, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            std::thread::sleep(Duration::from_micros(50));
        }
    }

    pub fn unlock_gate(&self) {
        self.gate.store(GATE_IDLE, Ordering::Release);
    }

    /// The container is gone: flag the instance and release a waiting audio thread.
    pub fn mark_lost(&self, status: InstanceStatus) {
        self.status.store(status as u32, Ordering::Release);
        if let Some(region) = self.region_ref.lock().unwrap().as_ref() {
            // The container no longer writes this word (it is dead), so the host may: the
            // waiter sees a value it did not expect and re-checks the status.
            let word = &region.header().rsp.seq;
            word.store(SEQ_ABORT, Ordering::SeqCst);
            stpc_transport::wake::address::wake(word);
        }
    }

    #[inline]
    fn default_deadline_ns(&self, frames: usize) -> u64 {
        let configured = self.deadline_ns.load(Ordering::Relaxed);
        if configured != 0 {
            return configured;
        }
        let rate = f64::from_bits(self.sample_rate_bits.load(Ordering::Relaxed)).max(1.0);
        ((frames as f64 / rate * 1e9) as u64).max(2_000_000)
    }

    /// Fallback without access to `RtState` (the gate is held by a control operation).
    ///
    /// # Safety
    /// As for [`Instance::process_raw`].
    unsafe fn fallback_plain(&self, block: &RawBlock, reason: u32) -> ProcessOutcome {
        self.fallback_blocks.fetch_add(1, Ordering::Relaxed);
        let frames = block.frames as usize;
        let mut silence = 0u64;
        for channel in 0..block.num_outputs as usize {
            let dst = *block.outputs.add(channel);
            if dst.is_null() {
                continue;
            }
            let src = if self.pass_through && channel < self.main_out && self.main_in > 0 {
                let source = channel.min(self.main_in - 1);
                if source < block.num_inputs as usize { *block.inputs.add(source) } else { std::ptr::null() }
            } else {
                std::ptr::null()
            };
            if src.is_null() {
                std::ptr::write_bytes(dst, 0, frames);
                silence |= 1u64 << (channel & 63);
            } else {
                std::ptr::copy(src, dst, frames); // may be the same buffer (in-place hosts)
            }
        }
        ProcessOutcome { flags: reason | PROCESS_FALLBACK, out_events: 0, output_silence: silence, plugin_ns: 0 }
    }

    /// Fallback with the latency-matched dry signal when the delay line is enabled.
    ///
    /// # Safety
    /// As for [`Instance::process_raw`]; the caller holds the gate and has fed the delay line.
    unsafe fn fallback(&self, rt: &RtState, block: &RawBlock, reason: u32) -> ProcessOutcome {
        let delay = self.latency.load(Ordering::Relaxed) as usize;
        if !(self.pass_through && rt.dry.enabled() && delay > 0) {
            return self.fallback_plain(block, reason);
        }
        self.fallback_blocks.fetch_add(1, Ordering::Relaxed);
        let frames = block.frames as usize;
        let mut silence = 0u64;
        for channel in 0..block.num_outputs as usize {
            let dst = *block.outputs.add(channel);
            if dst.is_null() {
                continue;
            }
            if channel < self.main_out && self.main_in > 0 {
                rt.dry.read(channel.min(self.main_in - 1), delay, frames, dst);
            } else {
                std::ptr::write_bytes(dst, 0, frames);
                silence |= 1u64 << (channel & 63);
            }
        }
        ProcessOutcome { flags: reason | PROCESS_FALLBACK, out_events: 0, output_silence: silence, plugin_ns: 0 }
    }

    /// # Safety
    /// As for [`Instance::process_raw`]; the caller holds the gate (PROCESSING).
    unsafe fn process_locked(&self, block: &RawBlock) -> ProcessOutcome {
        let rt = &mut *self.rt.get();
        let frames = block.frames as usize;
        self.blocks.fetch_add(1, Ordering::Relaxed);
        if frames == 0 {
            return ProcessOutcome { flags: PROCESS_OK, ..ProcessOutcome::default() };
        }
        if frames > rt.max_frames {
            return self.fallback_plain(block, PROCESS_INVALID);
        }
        if rt.dry.enabled() {
            rt.dry.feed(block.inputs, block.num_inputs as usize, frames);
        }
        if self.status.load(Ordering::Acquire) > InstanceStatus::Late as u32 {
            return self.fallback(rt, block, PROCESS_LOST);
        }
        let Some(link) = rt.link.as_ref() else {
            return self.fallback(rt, block, PROCESS_LOST);
        };
        if rt.peer_gone {
            // The supervisor is about to flag the instance; nothing will answer meanwhile.
            return self.fallback(rt, block, PROCESS_LOST);
        }
        if !self.active.load(Ordering::Relaxed) {
            return self.fallback(rt, block, PROCESS_INACTIVE);
        }
        if self.client_bypass.load(Ordering::Relaxed) {
            return self.fallback(rt, block, PROCESS_BYPASSED);
        }
        let region = &*link.region;
        let header = region.header();
        let mut flags = 0;

        if rt.pending != 0 {
            if header.rsp.seq.load(Ordering::SeqCst) != rt.pending {
                // Still busy with the block it missed: its buffers are not ours to touch.
                return self.fallback(rt, block, PROCESS_LATE);
            }
            rt.pending = 0;
            rt.discontinuity = true;
            self.late_since_ns.store(0, Ordering::Release);
            let _ = self.status.compare_exchange(
                InstanceStatus::Late as u32,
                InstanceStatus::Running as u32,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
            self.resyncs.fetch_add(1, Ordering::Relaxed);
            flags |= PROCESS_RESYNCED;
        }

        // ---- publish the request ----
        let mut input_silence = 0u64;
        for channel in 0..self.total_in {
            let src = if channel < block.num_inputs as usize { *block.inputs.add(channel) } else { std::ptr::null() };
            let dst = region.input(channel);
            if src.is_null() {
                std::ptr::write_bytes(dst, 0, frames);
                input_silence |= 1u64 << channel;
            } else {
                std::ptr::copy_nonoverlapping(src, dst, frames);
            }
        }
        let mut num_events = block.num_events as usize;
        if num_events > rt.event_capacity {
            num_events = rt.event_capacity;
            flags |= PROCESS_EVENTS_DROPPED;
        }
        let events = region.events();
        let last_frame = (frames - 1) as u32;
        for index in 0..num_events {
            let mut event = *block.events.add(index);
            event.offset = event.offset.min(last_frame);
            events.add(index).write(event);
        }
        let mut req_flags = 0;
        let offline = self.offline.load(Ordering::Relaxed);
        if offline {
            req_flags |= REQ_OFFLINE;
        }
        if rt.discontinuity {
            req_flags |= REQ_DISCONTINUITY;
            rt.discontinuity = false;
        }
        header.req.frames.store(frames as u32, Ordering::Relaxed);
        header.req.flags.store(req_flags, Ordering::Relaxed);
        header.req.num_events.store(num_events as u32, Ordering::Relaxed);
        header.req.input_silence.store(input_silence, Ordering::Relaxed);
        if block.transport.is_null() {
            header.req.set_transport(&Transport::default());
        } else {
            header.req.set_transport(&*block.transport);
        }
        rt.seq = next_seq(rt.seq);
        let seq = rt.seq;
        let started = monotonic_ns();
        self.wait_started_ns.store(started, Ordering::Relaxed);
        header.req.seq.store(seq, Ordering::SeqCst);
        link.notifier.notify(&header.req.seq);

        // ---- wait for the response ----
        let deadline = if offline {
            None
        } else {
            match block.deadline_ns {
                0 => Some(self.default_deadline_ns(frames)),
                DEADLINE_NONE => None,
                ns => Some(ns),
            }
        };
        #[derive(PartialEq)]
        enum Wait {
            Done,
            Late,
            Lost,
        }
        let result = loop {
            let seen = header.rsp.seq.load(Ordering::SeqCst);
            if seen == seq {
                break Wait::Done;
            }
            if seen == SEQ_ABORT || self.status.load(Ordering::Acquire) > InstanceStatus::Late as u32 {
                break Wait::Lost;
            }
            let elapsed = monotonic_ns() - started;
            if elapsed < self.spin_ns {
                std::hint::spin_loop();
                continue;
            }
            // Sleep in slices of at most 100 ms so that "no deadline" still notices a
            // container that died without the wake-up reaching us.
            let slice = match deadline {
                Some(limit) if elapsed >= limit => break Wait::Late,
                Some(limit) => (limit - elapsed).min(100_000_000),
                None => 100_000_000,
            };
            if link.waiter.wait(&header.rsp.seq, seen, Some(slice)) == WaitOutcome::PeerGone {
                // Pipe mode: the container closed its end, so it is dead or dying. The
                // supervisor flags the instance once it has reaped the process; until
                // then this attachment answers "lost" by itself.
                rt.peer_gone = true;
                break Wait::Lost;
            }
        };
        self.wait_started_ns.store(0, Ordering::Relaxed);

        match result {
            Wait::Lost => self.fallback(rt, block, PROCESS_LOST),
            Wait::Late => {
                rt.pending = seq;
                self.late_seq.store(seq, Ordering::Relaxed);
                self.late_since_ns.store(monotonic_ns().max(1), Ordering::Release);
                let _ = self.status.compare_exchange(
                    InstanceStatus::Running as u32,
                    InstanceStatus::Late as u32,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                );
                self.deadline_misses.fetch_add(1, Ordering::Relaxed);
                self.fallback(rt, block, flags | PROCESS_LATE)
            }
            Wait::Done => {
                let rsp_flags = header.rsp.flags.load(Ordering::Relaxed);
                if rsp_flags & RSP_PROCESSED == 0 {
                    let reason = if rsp_flags & RSP_INACTIVE != 0 {
                        PROCESS_INACTIVE
                    } else if rsp_flags & RSP_BUSY != 0 {
                        PROCESS_BUSY
                    } else {
                        PROCESS_PLUGIN_ERROR
                    };
                    return self.fallback(rt, block, flags | reason);
                }
                for channel in 0..self.total_out.min(block.num_outputs as usize) {
                    let dst = *block.outputs.add(channel);
                    if !dst.is_null() {
                        std::ptr::copy_nonoverlapping(region.output(channel) as *const f32, dst, frames);
                    }
                }
                for channel in self.total_out..block.num_outputs as usize {
                    let dst = *block.outputs.add(channel);
                    if !dst.is_null() {
                        std::ptr::write_bytes(dst, 0, frames);
                    }
                }
                // Counts come from memory the plug-in process can scribble on: clamp.
                let produced = (header.rsp.num_out_events.load(Ordering::Relaxed) as usize).min(rt.out_event_capacity);
                let taken = produced.min(block.out_event_capacity as usize);
                if taken > 0 {
                    std::ptr::copy_nonoverlapping(region.out_events() as *const Event, block.out_events, taken);
                }
                if taken < produced || rsp_flags & RSP_OUT_EVENTS_DROPPED != 0 {
                    flags |= PROCESS_OUT_EVENTS_DROPPED;
                }
                if rsp_flags & RSP_ERROR != 0 {
                    flags |= PROCESS_PLUGIN_ERROR;
                }
                ProcessOutcome {
                    flags: flags | PROCESS_OK,
                    out_events: taken,
                    output_silence: header.rsp.output_silence.load(Ordering::Relaxed),
                    plugin_ns: header.rsp.process_ns.load(Ordering::Relaxed),
                }
            }
        }
    }
}

/// A plug-in instance living in a container process. Dropping it destroys the plug-in
/// (do that on a control thread, never on the audio thread).
///
/// `process`/`process_raw` are for one audio thread at a time; all other methods are
/// blocking control calls and may be used from any thread concurrently with `process`.
pub struct Instance {
    pub(crate) shared: Arc<InstanceShared>,
    pub(crate) sup: Arc<SupInner>,
}

impl Instance {
    pub fn id(&self) -> InstanceId {
        self.shared.id
    }

    /// Real-time safe.
    pub fn status(&self) -> InstanceStatus {
        self.shared.status()
    }

    /// The plug-in's latency in frames, as last reported. Real-time safe. The transport
    /// itself adds none: a block is processed within the `process` call that submits it.
    pub fn latency(&self) -> u32 {
        self.shared.latency.load(Ordering::Relaxed)
    }

    /// The plug-in's tail in frames (`u64::MAX`: infinite). Real-time safe.
    pub fn tail(&self) -> u64 {
        self.shared.tail.load(Ordering::Relaxed)
    }

    /// Real-time safe.
    pub fn stats(&self) -> InstanceStats {
        let dropped = self
            .shared
            .region_ref
            .try_lock()
            .ok()
            .and_then(|region| region.as_ref().map(|r| r.dropped_notifications() as u64))
            .unwrap_or(0);
        InstanceStats {
            blocks: self.shared.blocks.load(Ordering::Relaxed),
            fallback_blocks: self.shared.fallback_blocks.load(Ordering::Relaxed),
            deadline_misses: self.shared.deadline_misses.load(Ordering::Relaxed),
            resyncs: self.shared.resyncs.load(Ordering::Relaxed),
            dropped_notifications: dropped,
        }
    }

    /// Changes the deadline `process` applies (`None`: automatic). Real-time safe.
    pub fn set_deadline(&self, deadline: Option<Duration>) {
        self.shared.deadline_ns.store(deadline.map_or(0, |d| (d.as_nanos() as u64).max(1)), Ordering::Relaxed);
    }

    /// Processes one block on the audio thread: real-time safe, synchronous, bounded by
    /// the deadline. Always fills the outputs: with the plug-in's audio
    /// ([`ProcessOutcome::processed`]) or with the fallback.
    pub fn process(&self, io: &mut ProcessBuffers<'_, '_>) -> ProcessOutcome {
        let frames = io.frames;
        let mut inputs = [std::ptr::null::<f32>(); MAX_CHANNELS];
        let mut outputs = [std::ptr::null_mut::<f32>(); MAX_CHANNELS];
        let num_inputs = io.inputs.len().min(MAX_CHANNELS);
        let num_outputs = io.outputs.len().min(MAX_CHANNELS);
        for (slot, channel) in inputs.iter_mut().zip(io.inputs.iter()) {
            if channel.len() >= frames {
                *slot = channel.as_ptr();
            }
        }
        let mut invalid = frames > u32::MAX as usize;
        for (slot, channel) in outputs.iter_mut().zip(io.outputs.iter_mut()) {
            if channel.len() >= frames {
                *slot = channel.as_mut_ptr();
            } else {
                channel.fill(0.0);
                invalid = true;
            }
        }
        if invalid {
            for channel in io.outputs.iter_mut() {
                let len = channel.len().min(frames);
                channel[..len].fill(0.0);
            }
            return ProcessOutcome { flags: PROCESS_INVALID | PROCESS_FALLBACK, ..ProcessOutcome::default() };
        }
        let block = RawBlock {
            frames: frames as u32,
            inputs: inputs.as_ptr(),
            num_inputs: num_inputs as u32,
            outputs: outputs.as_ptr(),
            num_outputs: num_outputs as u32,
            events: io.events.as_ptr(),
            num_events: io.events.len().min(u32::MAX as usize) as u32,
            out_events: io.out_events.as_mut_ptr(),
            out_event_capacity: io.out_events.len().min(u32::MAX as usize) as u32,
            transport: io.transport.map_or(std::ptr::null(), |t| t as *const Transport),
            deadline_ns: io.deadline.map_or(0, |d| (d.as_nanos() as u64).max(1)),
        };
        // SAFETY: every pointer comes from a live slice of at least the stated length.
        unsafe { self.process_raw(&block) }
    }

    /// [`Instance::process`] over raw channel pointers.
    ///
    /// # Safety
    /// `inputs`/`outputs` must hold `num_inputs`/`num_outputs` pointers, each null or valid
    /// for `frames` samples (outputs writable, not overlapping each other; an output may be
    /// the same buffer as an input). `events` must be valid for `num_events` events,
    /// `out_events` for `out_event_capacity`. `transport` must be null or valid.
    pub unsafe fn process_raw(&self, block: &RawBlock) -> ProcessOutcome {
        let shared = &*self.shared;
        if shared
            .gate
            .compare_exchange(GATE_IDLE, GATE_PROCESSING, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            shared.blocks.fetch_add(1, Ordering::Relaxed);
            return shared.fallback_plain(block, PROCESS_BUSY);
        }
        let outcome = shared.process_locked(block);
        shared.gate.store(GATE_IDLE, Ordering::Release);
        outcome
    }
}

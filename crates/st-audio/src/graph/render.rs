//! The renderer: one `render` call that pulls the engine and runs the current plan, for a device
//! callback and for an offline loop alike.
//!
//! # Who owns what
//!
//! A [`Renderer`] is the render side of one audio engine: the engine's render half, the plan in
//! use and the graph clock. Whoever calls [`Renderer::render`] is the audio thread for that
//! call; the type has no interior sharing, so `&mut self` is the whole real-time contract.
//! (Devices and the offline path reach it through a mutex they only `try_lock`, see
//! [`crate::device`].)
//!
//! # Hand-overs with the control thread ([`RenderShared`])
//!
//! | What | Direction | Mechanism |
//! |---|---|---|
//! | a new plan | control -> render | [`RenderShared::publish`] swaps a pointer into `pending`; the render takes it at its next block and calls `Plan::adopt` |
//! | the replaced plan | render -> control | pushed onto the `retired` ring; [`RenderShared::collect`] frees it (the render never frees a plan) |
//! | "the project changed" | control -> render | `automation_epoch` counter: the render resets automated values to the plan's defaults and asks the engine to re-send lanes |
//! | a knob turned | control -> render | `messages` ring of [`Message`]s, addressed by plan serial and target |
//! | silence everything (panic) | control -> render | `clear_tails` flag |
//! | meters | render -> reader | atomics in the plan's [`MeterBank`](super::meters::MeterBank) |
//! | a live input's audio | input thread -> render | the ring of its [`LineFeed`](crate::inputs::LineFeed), read by the plan |
//! | a tapped strip | render -> recorder | the rings of a [`StripTap`](super::tap::StripTap), stamped with the song position the engine's clock gives for the block |
//! | load, clock, counters | render -> anyone | atomics |
//!
//! Nothing here blocks: the render side uses atomic swaps and single-producer/single-consumer
//! rings only, and allocates nothing.

use super::plan::Plan;
use super::tap::NOT_PLAYING;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use tracker_engine::clock::{CLOCK_PLAYING, CLOCK_PREROLL};
use std::sync::Arc;
use tracker_engine::engine::abi::STEvent;
use tracker_engine::engine::host::{RenderHalf, StatusHandle};
use tracker_engine::engine::util::Ring;

/// A value for the render side that is not part of a plan.
#[derive(Clone, Copy, Default)]
pub struct Message {
    /// The plan the target index belongs to; ignored by any other plan (a newer plan already
    /// starts from the value).
    pub serial: u64,
    pub target: u32,
    pub address: u32,
    pub value: f32,
}

/// Receives the engine's note events for instruments the engine does not sound itself (plug-in
/// instruments, external MIDI). Called on the render thread: no allocation, no locks.
pub trait ExternalEvents: Send {
    /// `block_start` is the graph time of the block's first frame; the event carries its offset.
    fn event(&mut self, event: &STEvent, block_start: u64);
}

/// What runs on the mixed output, after the master strip: preview voices, the recording tap,
/// input monitoring ([`crate::live`]). Called on the render thread once per [`Renderer::render`]
/// call: no allocation, no locks, no frees.
pub trait OutputStage: Send {
    /// `time` is the graph time of the first frame of `left`/`right`.
    fn process(&mut self, time: u64, left: &mut [f32], right: &mut [f32]);
}

/// What the control side and the render side of one engine share.
pub struct RenderShared {
    pending: AtomicPtr<Plan>,
    /// Plans the render replaced, as `Box::into_raw` addresses.
    retired: Ring<usize>,
    automation_epoch: AtomicU64,
    clear_tails: AtomicBool,
    messages: Ring<Message>,
    /// Smoothed render time as a fraction of real time (`f32` bits).
    load: AtomicU32,
    frames: AtomicU64,
    adopted_serial: AtomicU64,
    latency_frames: AtomicU64,
    dropped_events: AtomicU64,
    leaked_plans: AtomicU64,
}

impl Default for RenderShared {
    fn default() -> Self {
        RenderShared::new()
    }
}

impl RenderShared {
    pub fn new() -> RenderShared {
        RenderShared {
            pending: AtomicPtr::new(std::ptr::null_mut()),
            retired: Ring::new(64),
            automation_epoch: AtomicU64::new(0),
            clear_tails: AtomicBool::new(false),
            messages: Ring::new(1024),
            load: AtomicU32::new(0),
            frames: AtomicU64::new(0),
            adopted_serial: AtomicU64::new(0),
            latency_frames: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            leaked_plans: AtomicU64::new(0),
        }
    }

    /// Control thread: make `plan` the one the render uses from its next block. A plan published
    /// earlier and not yet taken is replaced and freed here.
    pub fn publish(&self, plan: Box<Plan>) {
        self.collect();
        let old = self.pending.swap(Box::into_raw(plan), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: a non-null `pending` is a `Box::into_raw` from an earlier publish that the
            // render did not take (its swap would have left null); the swap returned it to us.
            drop(unsafe { Box::from_raw(old) });
        }
    }

    /// Control thread: free the plans the render has replaced.
    pub fn collect(&self) {
        while let Some(address) = self.retired.pop() {
            // SAFETY: every address on the ring is the `Box::into_raw` of a plan the render
            // replaced and pushed exactly once; it does not touch the plan afterwards.
            drop(unsafe { Box::from_raw(address as *mut Plan) });
        }
    }

    /// Control thread: the project changed; automated values return to their defaults until the
    /// engine re-sends them.
    pub fn automation_changed(&self) {
        self.automation_epoch.fetch_add(1, Ordering::Release);
    }

    /// Control thread: silence every tail at the next block (panic).
    pub fn request_clear_tails(&self) {
        self.clear_tails.store(true, Ordering::Release);
    }

    /// Control thread (the single producer). False when the ring is full.
    pub fn send(&self, message: Message) -> bool {
        self.messages.push(&message)
    }

    /// Render time as a fraction of real time, smoothed.
    pub fn load(&self) -> f32 {
        f32::from_bits(self.load.load(Ordering::Relaxed))
    }

    /// Frames rendered so far (the graph clock).
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// The serial of the plan the render uses.
    pub fn adopted_serial(&self) -> u64 {
        self.adopted_serial.load(Ordering::Relaxed)
    }

    /// Output latency of the plan in use, in frames.
    pub fn latency_frames(&self) -> u64 {
        self.latency_frames.load(Ordering::Relaxed)
    }

    /// Automation events dropped by full queues.
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }
}

impl Drop for RenderShared {
    fn drop(&mut self) {
        self.collect();
        let pending = self.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !pending.is_null() {
            // SAFETY: as in `publish`.
            drop(unsafe { Box::from_raw(pending) });
        }
    }
}

/// Flush-to-zero and denormals-are-zero for the duration of a render call, so decaying tails
/// never reach slow subnormal arithmetic. The caller's floating-point mode returns on drop.
struct DenormalGuard {
    #[cfg(target_arch = "x86_64")]
    saved: u32,
    #[cfg(target_arch = "aarch64")]
    saved: u64,
}

impl DenormalGuard {
    #[cfg(target_arch = "x86_64")]
    #[allow(deprecated)]
    fn new() -> DenormalGuard {
        // SAFETY: reading and writing MXCSR is always valid on x86-64 (SSE is baseline).
        unsafe {
            let saved = std::arch::x86_64::_mm_getcsr();
            std::arch::x86_64::_mm_setcsr(saved | 0x8040);
            DenormalGuard { saved }
        }
    }

    #[cfg(target_arch = "aarch64")]
    fn new() -> DenormalGuard {
        let saved: u64;
        // SAFETY: FPCR is readable and writable from user space; bit 24 is flush-to-zero.
        unsafe {
            std::arch::asm!("mrs {}, fpcr", out(reg) saved, options(nomem, nostack, preserves_flags));
            std::arch::asm!("msr fpcr, {}", in(reg) saved | (1u64 << 24), options(nomem, nostack, preserves_flags));
        }
        DenormalGuard { saved }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    fn new() -> DenormalGuard {
        DenormalGuard {}
    }
}

impl Drop for DenormalGuard {
    #[allow(deprecated)]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as in `new`.
        unsafe {
            std::arch::x86_64::_mm_setcsr(self.saved)
        };
        #[cfg(target_arch = "aarch64")]
        // SAFETY: as in `new`.
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) self.saved, options(nomem, nostack, preserves_flags))
        };
    }
}

/// The render side of one audio engine. See the module documentation.
pub struct Renderer {
    engine: RenderHalf,
    status: StatusHandle,
    shared: Arc<RenderShared>,
    plan: Box<Plan>,
    /// Graph time of the next frame.
    time: u64,
    seen_epoch: u64,
    external: Option<Box<dyn ExternalEvents>>,
    sample_rate: f64,
    /// MIDI output: the hook and its block state (see [`crate::midi`]).
    midi: crate::midi::RenderMidi,
    /// What runs on the output after the mixer (see [`OutputStage`]).
    output_stage: Option<Box<dyn OutputStage>>,
}

impl Renderer {
    /// `plan` is the plan to start with (an empty mixer's, usually).
    pub fn new(engine: RenderHalf, status: StatusHandle, shared: Arc<RenderShared>, mut plan: Box<Plan>) -> Renderer {
        plan.adopt();
        shared.adopted_serial.store(plan.serial, Ordering::Relaxed);
        shared.latency_frames.store(plan.latency_frames, Ordering::Relaxed);
        let sample_rate = engine.sample_rate();
        Renderer { engine, status, shared, plan, time: 0, seen_epoch: 0, external: None, sample_rate, midi: Default::default(), output_stage: None }
    }

    /// The MIDI output hook of this renderer (set, suspend for offline work).
    pub fn midi(&mut self) -> &mut crate::midi::RenderMidi {
        &mut self.midi
    }

    /// Where the engine's note events for external instruments go (plug-in instruments, MIDI
    /// outputs); `None` drops them.
    pub fn set_external_events(&mut self, sink: Option<Box<dyn ExternalEvents>>) {
        self.external = sink;
    }

    /// The plan in use, for the thread that drives this renderer offline (stem capture reads
    /// the strips' signals between render calls).
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// What runs on the output after the mixer; `None` for nothing (offline renders). The
    /// stage replaced is returned, to be dropped by the caller on a control thread.
    pub fn set_output_stage(&mut self, stage: Option<Box<dyn OutputStage>>) -> Option<Box<dyn OutputStage>> {
        std::mem::replace(&mut self.output_stage, stage)
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Graph time of the next frame to render.
    pub fn time(&self) -> u64 {
        self.time
    }

    fn adopt_pending(&mut self) {
        let next = self.shared.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if next.is_null() {
            return;
        }
        // SAFETY: a non-null `pending` is the `Box::into_raw` of a plan published for us; the
        // swap made this thread its only owner.
        let next = unsafe { Box::from_raw(next) };
        let old = std::mem::replace(&mut self.plan, next);
        self.plan.adopt();
        self.shared.dropped_events.fetch_add(old.dropped_events(), Ordering::Relaxed);
        self.shared.adopted_serial.store(self.plan.serial, Ordering::Relaxed);
        self.shared.latency_frames.store(self.plan.latency_frames, Ordering::Relaxed);
        // The old plan goes back to the control thread, which frees it. Should the ring be full
        // (the control thread collects before every publish, so it cannot be), the plan is
        // leaked rather than freed here.
        let address = Box::into_raw(old) as usize;
        if !self.shared.retired.push(&address) {
            self.shared.leaked_plans.fetch_add(1, Ordering::Relaxed);
        }
        // The new plan starts from the project's values: have the engine re-send what is automated.
        self.status.refresh_automation();
    }

    /// The transport at the start of the block the engine just rendered (`count` frames).
    fn block_info(&self, count: usize, cycle_end_ns: u64) -> super::insert::BlockInfo {
        let status = self.status.status();
        let playing = status.playing != 0 && status.count_in == 0;
        // The engine reports the position after the block; a playing block started `count` frames before it.
        let (beats, seconds) = if playing { (count as f64 * status.bpm / (60.0 * self.sample_rate), count as f64 / self.sample_rate) } else { (0.0, 0.0) };
        super::insert::BlockInfo {
            playing,
            recording: status.recording != 0,
            bpm: status.bpm,
            numerator: status.numerator.clamp(1, 64) as u16,
            denominator: status.denominator.clamp(1, 64) as u16,
            sample_position: ((status.seconds - seconds).max(0.0) * self.sample_rate).round() as i64,
            beat: (status.beat - beats).max(0.0),
            bar_start_beat: status.bar_start_beat.min((status.beat - beats).max(0.0)),
            cycle_end_ns,
        }
    }

    /// Renders `left.len()` frames (both slices must have that length) and advances the clock.
    /// Real-time safe: no allocation, no lock, no system call other than reading the clock.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        if frames == 0 {
            return;
        }
        let _denormals = DenormalGuard::new();
        let started = std::time::Instant::now();
        // Plug-ins in containers get the time left in this call as their deadline.
        let cycle_end_ns = super::insert::monotonic_ns() + (frames as f64 / self.sample_rate * 1e9) as u64;
        let stage_time = self.time;
        let mut at = 0;
        while at < frames {
            self.adopt_pending();
            let epoch = self.shared.automation_epoch.load(Ordering::Acquire);
            if epoch != self.seen_epoch {
                self.seen_epoch = epoch;
                self.plan.reset_automation();
                self.status.refresh_automation();
            }
            if self.shared.clear_tails.swap(false, Ordering::AcqRel) {
                self.plan.reset_signal();
                self.midi.silence();
            }
            let wanted = (frames - at).min(self.plan.max_frames());
            let count = self.engine.render_block(self.time, wanted as u32) as usize;
            if count == 0 {
                left[at..].fill(0.0);
                right[at..].fill(0.0);
                break;
            }
            self.plan.begin_block(self.time, count);
            self.plan.set_block(self.block_info(count, cycle_end_ns));
            if self.plan.wants_song_frame() {
                // Where in the song the engine was at the block's first frame, exactly.
                let song_frame = match self.engine.clock_span(self.time as i64, 1, 0) {
                    Ok(span) if span.context.flags & CLOCK_PLAYING != 0 && span.context.flags & CLOCK_PREROLL == 0 => span.context.song_frame,
                    _ => NOT_PLAYING,
                };
                self.plan.set_song_frame(song_frame);
            }
            self.midi.begin_block(&self.status, self.time, count, self.sample_rate);
            while let Some(message) = self.shared.messages.pop() {
                if message.serial == self.plan.serial {
                    self.plan.live_parameter(message.target, message.address, message.value, self.time);
                }
            }
            while let Some(event) = self.engine.pop_event() {
                self.plan.engine_event(&event, self.time);
                self.midi.event(&event);
                if let Some(external) = self.external.as_mut() {
                    external.event(&event, self.time);
                }
            }
            let block = self.engine.block();
            self.plan.process(&block, count, &mut left[at..at + count], &mut right[at..at + count]);
            self.time += count as u64;
            at += count;
        }
        if let Some(stage) = self.output_stage.as_mut() {
            stage.process(stage_time, &mut left[..frames], &mut right[..frames]);
        }
        self.shared.frames.store(self.time, Ordering::Relaxed);
        let spent = started.elapsed().as_secs_f64() * self.sample_rate / frames as f64;
        let previous = self.shared.load();
        let load = if spent as f32 > previous { spent as f32 } else { previous + (spent as f32 - previous) * 0.1 };
        self.shared.load.store(load.to_bits(), Ordering::Relaxed);
    }
}

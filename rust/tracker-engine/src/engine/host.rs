//! The engine as a Rust host owns it: [`Engine`], its two thread halves and the status handle.
//!
//! # One engine, three handles
//!
//! [`Engine::new`] creates an engine; it is the only way to get one. An `Engine` owns the three
//! handles and suits a single thread (tests, offline tools). [`Engine::split`] takes it apart
//! for a host with threads ([`split`] does both steps):
//!
//! * [`ControlHalf`]: project edits, commit, transport and note commands. One per engine, not
//!   clonable, every operation needs `&mut self`: whoever holds it is the control thread.
//! * [`RenderHalf`]: rendering, bus history, clock queries, events. One per engine, not
//!   clonable, `&mut self`: whoever holds it is the audio thread.
//! * [`StatusHandle`]: status, peaks, the voice playhead snapshot and the request mailboxes.
//!   Clonable; any thread.
//!
//! The engine is destroyed when the last of the three handles is dropped, which by then is the
//! only thread that can still reach it.
//!
//! # Why the handles may cross threads (the safety argument)
//!
//! The engine's storage (`EngineCore`) holds the control state and the render state in two
//! `UnsafeCell`s next to [`Shared`], and is `Send + Sync` by an `unsafe impl` (in `state.rs`).
//! That is sound because of what the rest of the crate lets safe code do with it:
//!
//! 1. **Exclusive halves.** The storage is private to the crate and reachable only through the
//!    handles here. The control cell is dereferenced only by `ControlHalf::parts`, the render
//!    cell only by `RenderHalf::parts`, each behind `&mut self` of a handle that exists once
//!    per engine and cannot be cloned. So at most one `&mut Control` and one `&mut Render`
//!    exist at any time, each tied to the borrow of its handle. Neither [`Control`] nor
//!    [`Render`] is `Send` (they hold raw project pointers), and `Shared` is not `Sync`, so
//!    the references `parts` returns cannot leave the thread that holds the handle; only the
//!    handle itself moves.
//! 2. **What both halves touch is in `Shared`**, and is made for exactly these two threads:
//!    atomics; the atomic project hand-over (`pending`, `retired`), where ownership of a
//!    project moves with the pointer; single-producer/single-consumer rings whose producer and
//!    consumer are fixed to one half each (commands: control to render; events and input
//!    acknowledgements: render to control side consumers, see below); and seqlock-guarded
//!    snapshots. `Shared`'s fields and ring operations are private to the crate. What it offers
//!    publicly (status, peaks, voice positions, the mailboxes) is safe from any thread, which
//!    is all a `StatusHandle` exposes.
//! 3. **Ring ends are tied to a half.** Events are popped only by [`RenderHalf::pop_event`]
//!    (the audio thread drains them after the block that produced them), input acknowledgements
//!    only by [`ControlHalf::pop_input_ack`], commands are pushed only by `Control` methods and
//!    popped only by the render. Each needs `&mut` of its half, so every ring has one producer
//!    and one consumer.
//! 4. **No mixing of engines.** The methods of `Control` and `Render` take the `&Shared` that
//!    `parts` returns next to them. Handing them another engine's `Shared` would move projects
//!    (and the samples they point to) between engines and give a ring a second producer, so
//!    each engine has an identity and every such method checks it: a foreign `Shared` makes the
//!    call a no-op (silence, `false`, an error), never an exchange.
//! 5. **Raw pointers inside `Render`** (`current`, `tb`, the pinned timing map) refer to
//!    projects, tables and maps that the engine keeps alive until the render half has let go of
//!    them (`retired` list, `origin_pins`, sample reference counts); `Control::collect` frees
//!    only what was handed back. The fields are private to the crate, and the constructors of
//!    `Control`, `Render` and `Shared` are too, so safe code outside cannot forge that state.
//! 6. **Drop.** `EngineCore` is dropped by the last `Arc`, when no handle and therefore no
//!    borrow of either half exists; the dropping thread then stands in for both.
//!
//! One rule remains a documented contract rather than a type: [`RenderHalf::capture_transport`],
//! [`RenderHalf::restore_transport`] and [`RenderHalf::forget_external_voices`] are meant for a
//! stopped graph. Calling them between blocks is memory-safe (they need the render half, like
//! rendering) but changes what plays.

use super::abi::{ClockSpan, STEvent, STFreezeSpan, STInputAck, STStatus, STTransportState, STVoicePosition};
use super::control::Control;
use super::render_cache::SpanError;
use super::state::{Render, EngineCore, Shared};
use super::tables::{TablesState, MAX_BLOCK};
use super::util::cmin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

// SAFETY: see "Why the handles may cross threads" in this module's documentation. In short:
// `control` is dereferenced by the single `ControlHalf`, `render` by the single `RenderHalf`,
// each behind `&mut self`; everything in `shared` is atomics, seqlocks and
// single-producer/single-consumer rings whose ends are fixed to those two halves; the raw
// pointers inside `Render` refer to projects and tables the engine itself keeps alive.
unsafe impl Send for EngineCore {}
// SAFETY: as above; shared access never reaches the two `UnsafeCell`s without the owning half.
unsafe impl Sync for EngineCore {}

/// An engine: the sequencer, the sampler voices and their buses, at one sample rate.
///
/// Holds the three handles of a new engine. Use them in place ([`Engine::control`],
/// [`Engine::render`], [`Engine::status`]) on one thread, lend them to scoped threads
/// ([`Engine::halves`]), or take the engine apart with [`Engine::split`].
pub struct Engine {
    control: ControlHalf,
    render: RenderHalf,
    status: StatusHandle,
}

/// The control-thread half: project edits, commit, transport and note commands.
pub struct ControlHalf {
    engine: Arc<EngineCore>,
}

/// The render half: whoever holds it is the engine's audio thread.
pub struct RenderHalf {
    engine: Arc<EngineCore>,
}

/// What any thread may read or request.
#[derive(Clone)]
pub struct StatusHandle {
    engine: Arc<EngineCore>,
}

impl Engine {
    /// Creates an engine. `None` for an unsupported sample rate (8 kHz ... 384 kHz) or when
    /// memory is exhausted.
    pub fn new(sample_rate: f64) -> Option<Engine> {
        let engine: Arc<EngineCore> = Arc::from(EngineCore::create(sample_rate)?);
        Some(Engine {
            control: ControlHalf { engine: engine.clone() },
            render: RenderHalf { engine: engine.clone() },
            status: StatusHandle { engine },
        })
    }

    /// The engine as its three handles, each free to move to its own thread.
    pub fn split(self) -> (ControlHalf, RenderHalf, StatusHandle) {
        (self.control, self.render, self.status)
    }

    /// The three handles at once, borrowed: for scoped threads, or for code that edits and
    /// renders in turn.
    pub fn halves(&mut self) -> (&mut ControlHalf, &mut RenderHalf, &StatusHandle) {
        (&mut self.control, &mut self.render, &self.status)
    }

    pub fn control(&mut self) -> &mut ControlHalf {
        &mut self.control
    }

    pub fn render(&mut self) -> &mut RenderHalf {
        &mut self.render
    }

    pub fn status(&self) -> &StatusHandle {
        &self.status
    }

    pub fn sample_rate(&self) -> f64 {
        self.status.sample_rate()
    }
}

/// Creates an engine and returns its handles: `Engine::new(sample_rate)?.split()`.
pub fn split(sample_rate: f64) -> Option<(ControlHalf, RenderHalf, StatusHandle)> {
    Engine::new(sample_rate).map(Engine::split)
}

impl ControlHalf {
    /// The control state with what its methods need next to it: `(control, shared, sample rate)`.
    pub fn parts(&mut self) -> (&mut Control, &Shared, f64) {
        // SAFETY: this is the only `ControlHalf` of the engine and it is borrowed exclusively,
        // so no other reference to the control state exists (point 1 of the module's safety
        // argument).
        let control = unsafe { &mut *self.engine.control.get() };
        (control, &self.engine.shared, self.engine.sample_rate)
    }

    /// Compile and publish the edits made since the last commit.
    pub fn commit(&mut self) -> Result<(), String> {
        let (control, shared, rate) = self.parts();
        control.commit(shared, rate)
    }

    /// The next input acknowledgement, if the render thread has produced one. The control half
    /// is the ring's single consumer.
    pub fn pop_input_ack(&mut self) -> Option<STInputAck> {
        self.engine.shared.pop_input_ack()
    }

    pub fn sample_rate(&self) -> f64 {
        self.engine.sample_rate
    }
}

impl RenderHalf {
    /// The render state and the shared state its methods take.
    pub fn parts(&mut self) -> (&mut Render, &Shared) {
        // SAFETY: this is the only `RenderHalf` of the engine and it is borrowed exclusively
        // (point 1 of the module's safety argument).
        let render = unsafe { &mut *self.engine.render.get() };
        (render, &self.engine.shared)
    }

    /// Direct stereo render of the whole mix, `left.len()` frames (the shorter length when the
    /// buffers differ). Standalone and offline use; invalidates the bus history.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = cmin(left.len(), right.len());
        let (render, shared) = self.parts();
        render.render(shared, Some(&mut left[..frames]), Some(&mut right[..frames]), frames as u32);
    }

    /// Advances the engine by `frames` frames without output: queued commands and a newly
    /// committed project are applied, events are produced. `advance(0)` only applies them.
    pub fn advance(&mut self, frames: u32) {
        let (render, shared) = self.parts();
        render.render(shared, None, None, frames);
    }

    /// See [`Render::render_bus`].
    pub fn render_bus(&mut self, bus: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        let (render, shared) = self.parts();
        render.render_bus(shared, bus, time, left, right);
    }

    /// See [`Render::render_channel`].
    pub fn render_channel(&mut self, channel: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        let (render, shared) = self.parts();
        render.render_channel(shared, channel, time, left, right);
    }

    /// See [`Render::prepare_history`].
    pub fn prepare_history(&mut self, time: u64, frames: u32) {
        let (render, shared) = self.parts();
        render.prepare_history(shared, time, frames);
    }

    /// Renders the next `frames` frames (at most [`MAX_BLOCK`]; more is cut) at graph time `time`
    /// into the per-bus block buffers and returns the frames rendered. Read the buses with
    /// [`RenderHalf::block`] before the next call. Events of the block ([`RenderHalf::pop_event`])
    /// carry frame offsets from the block's first frame.
    ///
    /// With host mixing on (`Control::set_host_mixing_tracks`) a track bus holds the track's raw
    /// instrument and clip signal and the return bus holds the metronome. Bus history for
    /// [`RenderHalf::render_bus`] is not kept by this call; clock history is.
    pub fn render_block(&mut self, time: u64, frames: u32) -> u32 {
        let (render, shared) = self.parts();
        render.render_block(shared, time, frames)
    }

    /// The buses of the block [`RenderHalf::render_block`] rendered last.
    pub fn block(&mut self) -> BlockView<'_> {
        let (render, _) = self.parts();
        // SAFETY: `tb` points to the tables of the project in use; they stay alive until the
        // next `refresh`, which needs `&mut self` and therefore cannot run while the view lives.
        BlockView { tables: unsafe { &*render.tb } }
    }

    /// The next engine event of the blocks rendered so far. The render half is the event
    /// ring's single consumer.
    pub fn pop_event(&mut self) -> Option<STEvent> {
        self.engine.shared.pop_event()
    }

    /// See [`Render::clock_span`].
    pub fn clock_span(&mut self, graph_frame: i64, requested: u32, latency: i64) -> Result<ClockSpan, SpanError> {
        let (render, shared) = self.parts();
        render.clock_span(shared, graph_frame, requested, latency)
    }

    /// See [`Render::freeze_span`].
    pub fn freeze_span(&mut self, graph_frame: i64, requested: u32, latency: i64) -> Result<STFreezeSpan, SpanError> {
        let (render, shared) = self.parts();
        render.freeze_span(shared, graph_frame, requested, latency)
    }

    /// A checkpoint of the transport and of everything that sounds (stopped graph).
    pub fn capture_transport(&mut self) -> STTransportState {
        let (render, shared) = self.parts();
        render.capture_transport(shared)
    }

    /// Returns to a checkpoint taken from this engine with the same project (stopped graph).
    pub fn restore_transport(&mut self, state: &STTransportState, resume: bool) {
        let (render, shared) = self.parts();
        render.restore_transport(shared, state, resume);
    }

    /// Ends every external (plug-in or MIDI) voice on `channel` without emitting its note-off
    /// and returns how many there were (stopped graph).
    pub fn forget_external_voices(&mut self, channel: i32) -> i32 {
        let (render, shared) = self.parts();
        render.forget_external_voices(shared, channel)
    }

    pub fn sample_rate(&self) -> f64 {
        self.engine.sample_rate
    }
}

/// Read access to the per-bus buffers of the block rendered last.
pub struct BlockView<'a> {
    tables: &'a TablesState,
}

impl BlockView<'_> {
    /// Tracks of the project that rendered the block.
    pub fn track_count(&self) -> usize {
        self.tables.track_count.max(0) as usize
    }

    /// One track's bus as `(left, right)`, `MAX_BLOCK` frames long, or `None` when nothing
    /// played on it in this block (its buffer then holds stale data).
    pub fn track(&self, track: usize) -> Option<(&[f32], &[f32])> {
        if track >= self.track_count() || self.tables.bus_touched.get(track).is_none_or(|&touched| touched == 0) {
            return None;
        }
        Some((self.tables.bus_left(track), self.tables.bus_right(track)))
    }

    /// The return bus: with host mixing, the metronome and count-in clicks. Always written.
    pub fn return_bus(&self) -> (&[f32], &[f32]) {
        let bus = self.tables.base.max(0) as usize;
        (self.tables.bus_left(bus), self.tables.bus_right(bus))
    }
}

impl StatusHandle {
    pub fn status(&self) -> STStatus {
        self.engine.shared.status()
    }

    /// The raw (pre-fader, with host mixing) peak of one track since the last block, decaying.
    pub fn track_peak(&self, track: i32) -> f32 {
        self.engine.shared.track_peak(track)
    }

    /// Ask the render thread to re-emit mixer defaults and automation lanes at its next block.
    pub fn refresh_automation(&self) {
        self.engine.shared.refresh_automation();
    }

    /// Start a fresh timestamp epoch for the bus history: the next pull at any time starts a
    /// new timeline. For a host whose graph stopped and restarts at another time.
    pub fn reset_render_cache(&self) {
        self.engine.shared.reset_render_cache();
    }

    pub fn watch_voice_positions(&self, instrument: i32) {
        self.engine.shared.watch_voice_positions(instrument);
    }

    pub fn voice_positions(&self, out: &mut [STVoicePosition]) -> Option<(usize, u64)> {
        self.engine.shared.voice_positions(out)
    }

    pub fn sample_rate(&self) -> f64 {
        self.engine.sample_rate
    }
}

impl Render {
    /// See [`RenderHalf::render_block`].
    pub fn render_block(&mut self, sh: &Shared, time: u64, frames: u32) -> u32 {
        if !self.owns(sh) {
            return 0;
        }
        let epoch = sh.cache_epoch.load(Ordering::Acquire);
        if self.observed_cache_epoch != epoch {
            self.cache_valid = false;
            self.observed_cache_epoch = epoch;
            self.reset_input_run();
        }
        // The host renders contiguous blocks; any other timestamp starts a new timeline, as an
        // explicit cache reset would.
        if !self.cache_valid || time != self.cache_end {
            self.cache_generation = self.cache_generation.wrapping_add(1);
            self.release_clock_origin();
            if !self.freeze_transition_pending || self.freeze_natural_tail {
                self.freeze_transition(super::abi::ST_FREEZE_CAUSE_GRAPH_RESET, true, false);
            }
            self.timing_epoch = self.timing_epoch.wrapping_add(1);
            self.cache_end = time;
            self.clock_begin = time;
            self.cache_valid = true;
        }
        let count = cmin(frames, MAX_BLOCK as u32);
        if count == 0 {
            return 0;
        }
        self.process(sh, None, None, count, true, 0, time);
        self.cache_end += count as u64;
        // No bus history is written here: nothing before `cache_end` can be read back.
        self.cache_begin = self.cache_end;
        if self.cache_end - self.clock_begin > super::tables::CLOCK_CAPACITY as u64 {
            self.clock_begin = self.cache_end - super::tables::CLOCK_CAPACITY as u64;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::abi::{default_instrument, default_track, STStep, ST_EVENT_PARAMETER, ST_MODE_FORWARD};
    use crate::engine::edits::Pcm;

    fn assert_send<T: Send>() {}

    #[test]
    fn the_halves_move_to_their_threads_and_the_block_render_exposes_every_bus() {
        assert_send::<ControlHalf>();
        assert_send::<RenderHalf>();
        assert_send::<StatusHandle>();
        assert!(split(1_000.0).is_none(), "an unsupported rate gives no engine");
        let (mut control, mut render, status) = split(48_000.0).expect("an engine");
        {
            let (edit, shared, rate) = control.parts();
            edit.begin_update(shared);
            edit.set_host_mixing_tracks(true, &[], true);
            assert!(edit.set_track_count(2) && edit.set_instrument_count(1));
            // A fader the host must not hear in the raw bus.
            edit.set_track(0, crate::engine::abi::STTrack { volume: 0.25, ..default_track() });
            let mut instrument = default_instrument();
            instrument.mode = ST_MODE_FORWARD;
            instrument.volume = 1.0;
            edit.set_instrument(0, instrument);
            let tone = vec![0.5f32; 4_800];
            assert!(edit.set_sample(shared, rate, 0, Some(Pcm { left: &tone, right: None, rate: 48_000.0 })));
            edit.set_pattern(0, 16);
            edit.set_step(0, 0, 0, STStep { note: 60, instrument: 0, velocity: 127, gate: 0.0, ..Default::default() }, None);
            edit.set_sequence(&[0], false, 0);
        }
        control.commit().expect("the project is accepted");
        {
            let (edit, shared, _) = control.parts();
            edit.play(shared, 0);
        }
        // The render half works on another thread while this one keeps the control half.
        let rendered = std::thread::spawn(move || {
            assert_eq!(render.render_block(0, 100_000), MAX_BLOCK as u32, "a block is at most MAX_BLOCK frames");
            let mut mix_events = 0;
            while let Some(event) = render.pop_event() {
                mix_events += (event.type_ == ST_EVENT_PARAMETER && event.frame_offset == 0) as usize;
            }
            let block = render.block();
            assert_eq!(block.track_count(), 2);
            let (left, right) = block.track(0).expect("the playing track's bus");
            let peak = left[..MAX_BLOCK].iter().chain(&right[..MAX_BLOCK]).fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            assert!(block.track(1).is_none(), "nothing played on the second track");
            assert!(block.track(2).is_none());
            assert!(block.return_bus().0[..MAX_BLOCK].iter().all(|sample| *sample == 0.0), "no metronome: a silent return bus");
            // Contiguous blocks continue the song; another timestamp starts a new timeline without failing.
            assert_eq!(render.render_block(MAX_BLOCK as u64, 256), 256);
            assert_eq!(render.render_block(1_000_000, 64), 64);
            (peak, mix_events)
        })
        .join()
        .expect("the render thread");
        assert!(rendered.0 > 0.3, "the bus holds the raw voice, not the faded one (peak {})", rendered.0);
        assert!(rendered.1 >= 12, "the mixer defaults of both tracks were sent as events");
        let state = status.status();
        assert!(state.playing != 0 && state.track_count == 2);
        assert!(status.track_peak(0) > 0.3);
        assert!(control.pop_input_ack().is_none());
        assert_eq!((control.sample_rate(), status.sample_rate()), (48_000.0, 48_000.0));
    }
}

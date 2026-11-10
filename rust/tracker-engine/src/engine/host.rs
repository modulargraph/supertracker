//! The engine for a Rust host: the two thread halves as owned, safe values, and a block render
//! that leaves every bus in place for a host that mixes them itself.
//!
//! The C API hands out one `*mut STEngine` and relies on its callers to keep control functions
//! on one thread and render functions on another. A Rust host gets that rule from the type
//! system instead: [`split`] returns one [`ControlHalf`] and one [`RenderHalf`]. Neither can be
//! cloned and both need `&mut self`, so each half of the engine has exactly one user at a time;
//! each may move to (and stay on) its own thread. [`StatusHandle`] is the part any thread may
//! read: status, peaks, the voice playhead snapshot and the automation refresh mailbox.
//!
//! The engine is destroyed when the last of the three handles is dropped, which by then is the
//! only thread that can still reach it.

use super::abi::{STEvent, STInputAck, STStatus, STVoicePosition};
use super::control::Control;
use super::state::{Render, STEngine, Shared};
use super::tables::{TablesState, MAX_BLOCK};
use super::util::cmin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

// SAFETY: an `STEngine` is only reachable through the handles below (or the C API, whose
// contract is the same): `control` is dereferenced by the single `ControlHalf`, `render` by the
// single `RenderHalf`, each behind `&mut self`; everything in `shared` is atomics and
// single-producer/single-consumer rings whose producer and consumer are those two halves. The
// raw pointers inside `Render` refer to projects and tables that the engine itself keeps alive.
unsafe impl Send for STEngine {}
// SAFETY: as above; shared access never reaches the two `UnsafeCell`s without the owning half.
unsafe impl Sync for STEngine {}

/// The control-thread half: project edits, commit, transport and note commands.
pub struct ControlHalf {
    engine: Arc<STEngine>,
}

/// The render half: whoever holds it is the engine's audio thread.
pub struct RenderHalf {
    engine: Arc<STEngine>,
}

/// What any thread may read or request.
#[derive(Clone)]
pub struct StatusHandle {
    engine: Arc<STEngine>,
}

/// Creates an engine and returns its halves. `None` for an unsupported sample rate
/// (8 kHz ... 384 kHz) or when memory is exhausted.
pub fn split(sample_rate: f64) -> Option<(ControlHalf, RenderHalf, StatusHandle)> {
    let engine: Arc<STEngine> = Arc::from(STEngine::create(sample_rate)?);
    Some((ControlHalf { engine: engine.clone() }, RenderHalf { engine: engine.clone() }, StatusHandle { engine }))
}

impl ControlHalf {
    /// The control state with what its methods need next to it: `(control, shared, sample rate)`.
    pub fn parts(&mut self) -> (&mut Control, &Shared, f64) {
        // SAFETY: this is the only `ControlHalf` of the engine and it is borrowed exclusively,
        // so no other reference to the control state exists (see the `Send`/`Sync` note above).
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
        // SAFETY: this is the only `RenderHalf` of the engine and it is borrowed exclusively.
        let render = unsafe { &mut *self.engine.render.get() };
        (render, &self.engine.shared)
    }

    /// Renders the next `frames` frames (at most [`MAX_BLOCK`]; more is cut) at graph time `time`
    /// into the per-bus block buffers and returns the frames rendered. Read the buses with
    /// [`RenderHalf::block`] before the next call. Events of the block ([`RenderHalf::pop_event`])
    /// carry frame offsets from the block's first frame.
    ///
    /// With host mixing on (`Control::set_host_mixing_tracks`) a track bus holds the track's raw
    /// instrument and clip signal and the return bus holds the metronome. Bus history for
    /// `Render::render_cached` is not kept by this call; clock history is.
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

    /// The next engine event of the blocks rendered so far.
    pub fn pop_event(&mut self) -> Option<STEvent> {
        self.engine.shared.events.pop()
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

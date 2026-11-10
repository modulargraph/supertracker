//! Per-channel render tables and retained bus history: C++ `TrackState`, `RatchetState`,
//! `LaneEventState`, `PluginRowLock`, `BusRing`, `PeakStore`, `FreezeHistory` and `ChannelTables`
//! from `TrackerEngine.cpp`.
//!
//! Everything here is allocated on the control thread (at commit or engine creation) and then
//! used by the render thread, which never allocates or frees it.

use super::abi::*;
use super::project::PluginStepFX;
use super::util::FloatBlock;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub const MAX_BLOCK: usize = 4096;
pub const CACHE_CAPACITY: usize = 65536;
pub const CLOCK_CAPACITY: usize = ST_ENGINE_CLOCK_HISTORY_FRAMES;
/// `ST_TARGET_LAST_INSTRUMENT - ST_TARGET_ATTACK + 1`
pub const INSTRUMENT_TARGETS: usize = (ST_TARGET_LAST_INSTRUMENT - ST_TARGET_ATTACK + 1) as usize;

/// Per-bus block buffers are written one sample at a time across every touched bus. A
/// power-of-two stride (4096 floats = 16 KB) would put all buses' current sample in the same L1
/// cache set and thrash it; the padding staggers them across sets.
pub const BUS_STRIDE: usize = MAX_BLOCK + 48;

/// Sticky ratchet: the held note plus its resolved playback values (direction, offset, slice,
/// glide, arpeggio), so repeats on later rows replay it exactly.
#[derive(Clone, Copy)]
pub struct RatchetState {
    pub known: bool,
    pub handled: bool,
    pub rate: i32,
    pub instrument: i32,
    pub count: i32,
    pub reverse: i32,
    pub arp: i32,
    pub slice: i32,
    pub offset: f32,
    pub glide: f32,
    pub step: STStep,
    pub chord: [i32; ST_MAX_CHORD],
}

impl Default for RatchetState {
    fn default() -> Self {
        RatchetState {
            known: false,
            handled: false,
            rate: 1,
            instrument: 0,
            count: 1,
            reverse: -1,
            arp: 0,
            slice: -1,
            offset: 0.0,
            glide: 0.0,
            step: STStep::default(),
            chord: [0; ST_MAX_CHORD],
        }
    }
}

/// Effective mixer and instrument-automation values of one channel.
#[derive(Clone, Copy)]
pub struct TrackState {
    pub volume: f32,
    pub pan: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub delay: f32,
    pub reverb: f32,
    pub pitch: f32,
    /// Automated instrument targets (index: target - ST_TARGET_ATTACK); NaN = not automated.
    pub instrument: [f32; INSTRUMENT_TARGETS],
    pub row_locks: u64,
    pub instrument_mask: u64,
}

impl Default for TrackState {
    fn default() -> Self {
        TrackState {
            volume: 0.8,
            pan: 0.0,
            cutoff: -1.0,
            resonance: -1.0,
            delay: 0.0,
            reverb: 0.0,
            pitch: 0.0,
            instrument: [f32::NAN; INSTRUMENT_TARGETS],
            row_locks: 0,
            instrument_mask: 0,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct LaneEventState {
    pub value: f64,
    pub known: bool,
    pub delivered: bool,
    pub retry_frame: u64,
}

#[derive(Clone, Copy, Default)]
pub struct PluginRowLock {
    pub fx: PluginStepFX,
    pub track: i32,
    pub delivery: LaneEventState,
}

/// `BusRing::silentFrom` value of a bus whose ring holds real audio up to the cache end.
pub const AUDIBLE: u64 = u64::MAX;

pub struct BusRingState {
    pub l: FloatBlock,
    pub r: FloatBlock,
    /// Graph frames >= it read as zeros (the ring is not written while silent).
    pub silent_from: u64,
    /// The cache epoch the ring belongs to; any other epoch reads as silence.
    pub generation: u64,
}

/// Retained render_bus history for one logical bus (a track or the return). Identity follows the
/// logical bus, not its number, so history survives renumbering.
pub struct BusRing {
    state: UnsafeCell<BusRingState>,
}

// SAFETY: the control thread only creates rings, clones and drops their `Arc`s; the contents
// (`state`) are read and written by the render thread alone (see `state_mut`).
unsafe impl Send for BusRing {}
unsafe impl Sync for BusRing {}

impl BusRing {
    /// Control thread. `None` when memory is exhausted (C++ `std::bad_alloc`).
    pub fn new() -> Option<BusRing> {
        Some(BusRing {
            state: UnsafeCell::new(BusRingState {
                l: FloatBlock::new(CACHE_CAPACITY)?,
                r: FloatBlock::new(CACHE_CAPACITY)?,
                silent_from: 0,
                generation: 0,
            }),
        })
    }

    /// Render thread only.
    ///
    /// # Safety
    /// The caller must be the render thread (or an exclusive stopped-graph operation) and must
    /// not hold another reference to this ring's state.
    #[allow(clippy::mut_from_ref)]
    #[inline(always)]
    pub unsafe fn state_mut(&self) -> &mut BusRingState {
        // SAFETY: per this function's contract there is exactly one accessor.
        unsafe { &mut *self.state.get() }
    }
}

/// Peak meters are read from any thread. Storage only grows and old stores live until the engine
/// is destroyed, so a reader holding an old pointer stays valid. Values are `f32` bit patterns.
pub struct PeakStore {
    pub values: Box<[AtomicU32]>,
}

impl PeakStore {
    pub fn new(count: usize) -> PeakStore {
        PeakStore { values: (0..count).map(|_| AtomicU32::new(0f32.to_bits())).collect() }
    }
    #[inline(always)]
    pub fn load(&self, index: usize) -> f32 {
        match self.values.get(index) {
            Some(value) => f32::from_bits(value.load(Ordering::Relaxed)),
            None => 0.0,
        }
    }
    #[inline(always)]
    pub fn store(&self, index: usize, value: f32) {
        if let Some(slot) = self.values.get(index) {
            slot.store(value.to_bits(), Ordering::Relaxed);
        }
    }
}

/// Retained per-frame freeze metadata, indexed by graph frame modulo the clock capacity.
pub struct FreezeHistory {
    pub frames: Box<[STFreezeContext]>,
}

impl FreezeHistory {
    pub fn new() -> FreezeHistory {
        let blank = STFreezeContext { contentGraphFrame: i64::MIN, ..STFreezeContext::default() };
        FreezeHistory { frames: vec![blank; CLOCK_CAPACITY].into_boxed_slice() }
    }
}

/// The render-owned contents of `ChannelTables`.
pub struct TablesState {
    pub track_count: i32,
    /// The return bus (`st_return_bus(track_count)`).
    pub base: i32,
    pub channels: i32,
    pub buses: i32,
    pub track_state: Vec<TrackState>,
    pub pitch_bend: Vec<f32>,
    pub mod_wheel: Vec<f32>,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub peaks: Vec<f32>,
    pub touched: Vec<u64>,
    /// Raw host mixing: whether each channel's bus carries its signal this block (enabled and
    /// computational).
    pub host_audible: Vec<u8>,
    pub lane_ratchets: Vec<RatchetState>,
    pub plugin_row_locks: Vec<PluginRowLock>,
    pub bus_l: FloatBlock,
    pub bus_r: FloatBlock,
    pub bus_touched: Vec<u8>,
    pub rings: Vec<Option<Arc<BusRing>>>,
}

impl TablesState {
    /// One bus's block buffers (left, right), `MAX_BLOCK` frames each.
    #[inline(always)]
    pub fn bus_pair(&mut self, bus: usize) -> (&mut [f32], &mut [f32]) {
        let start = bus * BUS_STRIDE;
        let l = self.bus_l.as_mut_slice().get_mut(start..start + MAX_BLOCK).unwrap_or(&mut []);
        let r = self.bus_r.as_mut_slice().get_mut(start..start + MAX_BLOCK).unwrap_or(&mut []);
        (l, r)
    }
    #[inline(always)]
    pub fn bus_left(&self, bus: usize) -> &[f32] {
        let start = bus * BUS_STRIDE;
        self.bus_l.as_slice().get(start..start + MAX_BLOCK).unwrap_or(&[])
    }
    #[inline(always)]
    pub fn bus_right(&self, bus: usize) -> &[f32] {
        let start = bus * BUS_STRIDE;
        self.bus_r.as_slice().get(start..start + MAX_BLOCK).unwrap_or(&[])
    }
}

/// Every per-channel table the render thread uses, sized for one published project. Built
/// (allocated) on the control thread at commit, reused while the shape is the same, and migrated
/// by the render thread when a project with a new shape is swapped in. The render thread never
/// allocates or frees these.
pub struct ChannelTables {
    pub track_count: i32,
    pub channels: i32,
    state: UnsafeCell<TablesState>,
}

// SAFETY: after construction the control thread reads only the immutable shape fields
// (`track_count`, `channels`) and clones/drops the `Arc`; `state` is accessed by the render
// thread alone (see `state_ptr`).
unsafe impl Send for ChannelTables {}
unsafe impl Sync for ChannelTables {}

impl ChannelTables {
    /// Control thread. `rings[bus]` is the history ring of each logical bus that has one.
    /// `None` when memory is exhausted (C++ `std::bad_alloc`).
    pub fn new(tracks: i32, rings: Vec<Option<Arc<BusRing>>>) -> Option<ChannelTables> {
        let base = st_return_bus(tracks);
        let channels = st_channel_count(tracks);
        let buses = st_output_bus_count(tracks);
        let channel_count = channels as usize;
        let bus_count = buses as usize;
        Some(ChannelTables {
            track_count: tracks,
            channels,
            state: UnsafeCell::new(TablesState {
                track_count: tracks,
                base,
                channels,
                buses,
                track_state: vec![TrackState::default(); channel_count],
                pitch_bend: vec![0.0; channel_count],
                mod_wheel: vec![0.0; channel_count],
                left: vec![0.0; channel_count],
                right: vec![0.0; channel_count],
                peaks: vec![0.0; channel_count],
                touched: vec![0; (channel_count + 63) / 64],
                host_audible: vec![0; channel_count],
                lane_ratchets: vec![RatchetState::default(); tracks as usize],
                plugin_row_locks: vec![PluginRowLock::default(); tracks as usize * 2],
                bus_l: FloatBlock::new(bus_count * BUS_STRIDE)?,
                bus_r: FloatBlock::new(bus_count * BUS_STRIDE)?,
                bus_touched: vec![0; bus_count],
                rings,
            }),
        })
    }

    #[inline]
    pub fn same_shape(&self, tracks: i32) -> bool {
        tracks == self.track_count
    }

    /// Pointer to the render-owned contents. Dereferenced only by the render thread (or by an
    /// exclusive stopped-graph operation standing in for it), one reference at a time.
    #[inline(always)]
    pub fn state_ptr(&self) -> *mut TablesState {
        self.state.get()
    }
}

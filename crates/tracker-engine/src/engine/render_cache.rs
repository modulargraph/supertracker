//! Timestamped bus history: `render_bus` / `render_channel` and the per-bus rings behind them.
//!
//! Serialized graph pulls may overlap and use different frame counts; only an uncached suffix
//! advances the sequencer and DSP, existing ranges are copied without advancing music twice.

use super::state::*;
use super::tables::*;
use super::util::{cmax, cmin};
use std::sync::atomic::Ordering;

/// Render thread. A bus that was silent since `silent_from` has stale ring contents there; zero
/// the still-readable part of that gap before writing real audio.
fn zero_ring(ring: &mut BusRingState, mut from: u64, to: u64) {
    while from < to {
        let index = (from % CACHE_CAPACITY as u64) as usize;
        let count = cmin(to - from, (CACHE_CAPACITY - index) as u64) as usize;
        if let Some(slots) = ring.l.as_mut_slice().get_mut(index..index + count) {
            slots.fill(0.0);
        }
        if let Some(slots) = ring.r.as_mut_slice().get_mut(index..index + count) {
            slots.fill(0.0);
        }
        from += count as u64;
    }
}

/// Copy `source` into the ring starting at graph frame `start`, wrapping at the capacity.
fn copy_into_ring(ring: &mut [f32], source: &[f32], start: u64) {
    let index = (start % CACHE_CAPACITY as u64) as usize;
    let first = cmin(source.len(), CACHE_CAPACITY - index);
    if let (Some(to), Some(from)) = (ring.get_mut(index..index + first), source.get(..first)) {
        to.copy_from_slice(from);
    }
    if first < source.len() {
        let rest = source.len() - first;
        if let (Some(to), Some(from)) = (ring.get_mut(..rest), source.get(first..)) {
            to.copy_from_slice(from);
        }
    }
}

/// Copy `count` frames starting at graph frame `start` out of the ring.
fn copy_from_ring(ring: &[f32], out: &mut [f32], start: u64) {
    let index = (start % CACHE_CAPACITY as u64) as usize;
    let first = cmin(out.len(), CACHE_CAPACITY - index);
    if let (Some(to), Some(from)) = (out.get_mut(..first), ring.get(index..index + first)) {
        to.copy_from_slice(from);
    }
    if first < out.len() {
        let rest = out.len() - first;
        if let (Some(to), Some(from)) = (out.get_mut(first..), ring.get(..rest)) {
            to.copy_from_slice(from);
        }
    }
}

fn silence(buffer: &mut Option<&mut [f32]>) {
    if let Some(out) = buffer.as_deref_mut() {
        out.fill(0.0);
    }
}

impl Render {
    fn write_ring(&self, ring: &mut BusRingState, l: &[f32], r: &[f32], start: u64) {
        if ring.generation != self.cache_generation {
            ring.generation = self.cache_generation;
            ring.silent_from = 0;
        }
        if ring.silent_from != AUDIBLE {
            zero_ring(ring, cmax(ring.silent_from, self.cache_begin), start);
            ring.silent_from = AUDIBLE;
        }
        copy_into_ring(ring.l.as_mut_slice(), l, start);
        copy_into_ring(ring.r.as_mut_slice(), r, start);
    }

    fn silence_ring(&self, ring: &mut BusRingState, start: u64) {
        if ring.generation != self.cache_generation {
            ring.generation = self.cache_generation;
            ring.silent_from = 0;
        } else if ring.silent_from == AUDIBLE {
            ring.silent_from = start;
        }
    }

    /// One output bus at graph time `time`, `left.len()` frames (the shorter length when the
    /// buffers differ): the bus history is extended up to the end of the request (rendering the
    /// project in blocks, with their events) and the requested range is copied out, so several
    /// pulls of the same range, in any order of buses, render it once. Bus numbers: a track's
    /// own number for the tracks, then the return bus at [`return_bus`].
    ///
    /// Silence, without rendering, for a negative bus, more frames than the history holds
    /// ([`CACHE_CAPACITY`]) and a range that overflows the timestamp. Zero frames do nothing.
    /// History that has been overwritten reads as silence, as does a bus the project lacks.
    pub fn render_bus(&mut self, sh: &Shared, bus: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        self.render_history(sh, bus, false, time, left, right);
    }

    /// As [`Render::render_bus`], addressed by logical channel: a track's own channel number,
    /// or any negative number for the return bus. The bus is resolved against the project that
    /// rendered the range, so a caller keyed by channel never reads another channel's bus
    /// across a renumbering; a channel the project lacks is silent.
    pub fn render_channel(&mut self, sh: &Shared, channel: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        self.render_history(sh, channel, true, time, left, right);
    }

    /// Extends the bus history to `time + frames` without copying anything out: what
    /// [`Render::render_bus`] does before it reads. For hosts that only need the events of a
    /// range.
    pub fn prepare_history(&mut self, sh: &Shared, time: u64, frames: u32) {
        if self.engine_id != sh.engine_id || frames == 0 || frames as usize > CACHE_CAPACITY || time > u64::MAX - frames as u64 {
            return;
        }
        // The return bus always exists; nothing is copied without output buffers.
        self.render_cached(sh, -1, true, time, None, None, frames);
    }

    /// The argument rules shared by `render_bus` and `render_channel`.
    fn render_history(&mut self, sh: &Shared, bus: i32, channel: bool, time: u64, left: &mut [f32], right: &mut [f32]) {
        let frames = cmin(left.len(), right.len());
        let (left, right) = (&mut left[..frames], &mut right[..frames]);
        if self.engine_id != sh.engine_id
            || (!channel && bus < 0)
            || frames > CACHE_CAPACITY
            || time > u64::MAX - frames as u64
        {
            left.fill(0.0);
            right.fill(0.0);
            return;
        }
        if frames != 0 {
            self.render_cached(sh, bus, channel, time, Some(left), Some(right), frames as u32);
        }
    }

    /// `renderCached`: one bus (or logical channel) of `n` frames at graph time `time`. The
    /// caller has already rejected `n > CACHE_CAPACITY`, a negative bus and timestamp overflow.
    pub(crate) fn render_cached(
        &mut self,
        sh: &Shared,
        mut bus: i32,
        channel: bool,
        time: u64,
        mut l: Option<&mut [f32]>,
        mut r: Option<&mut [f32]>,
        n: u32,
    ) {
        let epoch = sh.cache_epoch.load(Ordering::Acquire);
        if self.observed_cache_epoch != epoch {
            self.cache_valid = false;
            self.observed_cache_epoch = epoch;
            self.reset_input_run();
        }
        if !self.cache_valid {
            self.cache_generation = self.cache_generation.wrapping_add(1);
            self.cache_begin = time;
            self.cache_end = time;
            self.cache_valid = true;
        }
        let end = time + n as u64;
        // A discontinuity larger than retained history requires an explicit graph reset; do not
        // run an unbounded catch-up render on the real-time thread.
        if time > self.cache_end && time - self.cache_end > CACHE_CAPACITY as u64 {
            silence(&mut l);
            silence(&mut r);
            return;
        }
        while self.cache_end < end {
            let start = self.cache_end;
            let count = cmin(MAX_BLOCK as u64, end - start) as u32;
            let event_offset = if start >= time { (start - time) as i32 } else { -((time - start) as i32) };
            self.process(sh, None, None, count, true, event_offset, start);
            // SAFETY: `tb` points to the tables of the project `process` just used; the render
            // thread is their only accessor and no other reference to them is live here.
            let tb = unsafe { &*self.tb };
            let block = count as usize;
            for b in 0..tb.buses as usize {
                let Some(Some(ring)) = tb.rings.get(b) else { continue };
                // SAFETY: ring contents are touched by the render thread only, and this is the
                // only reference to this ring's state in the loop body.
                let ring = unsafe { ring.state_mut() };
                if tb.bus_touched.get(b).is_some_and(|&touched| touched != 0) {
                    if let (Some(bl), Some(br)) = (tb.bus_left(b).get(..block), tb.bus_right(b).get(..block)) {
                        self.write_ring(ring, bl, br, start);
                    }
                } else {
                    self.silence_ring(ring, start);
                }
            }
            self.cache_end += count as u64;
            if self.cache_end - self.cache_begin > CACHE_CAPACITY as u64 {
                self.cache_begin = self.cache_end - CACHE_CAPACITY as u64;
            }
        }
        // SAFETY: as above; the tables are those of the project in use.
        let tb = unsafe { &*self.tb };
        // Channel addressing resolves the bus with the tables just rendered, so a caller keyed
        // by channel never reads another channel's bus across a renumbering.
        if channel {
            bus = if bus < 0 {
                tb.base
            } else if bus < tb.channels {
                bus
            } else {
                -1
            };
        }
        if bus < 0 {
            silence(&mut l);
            silence(&mut r);
            return;
        }
        let lost = if time < self.cache_begin { cmin(n as u64, self.cache_begin - time) as u32 } else { 0 };
        let ring = if bus < tb.buses {
            match tb.rings.get(bus as usize) {
                // SAFETY: render thread only; the single reference to this ring's state here.
                Some(Some(ring)) => Some(unsafe { ring.state_mut() }),
                _ => None,
            }
        } else {
            None
        };
        // Frames at or after silentFrom (and any ring from another cache epoch) are silence.
        let from = time + lost as u64;
        let audible_end = match &ring {
            Some(ring) if ring.generation == self.cache_generation => cmin(cmax(ring.silent_from, from), end),
            _ => from,
        };
        let count = (audible_end - from) as usize;
        let lost = lost as usize;
        for (out, left) in [(l, true), (r, false)] {
            let Some(out) = out else { continue };
            if let Some(prefix) = out.get_mut(..lost) {
                prefix.fill(0.0);
            }
            if count != 0 {
                if let (Some(ring), Some(target)) = (&ring, out.get_mut(lost..lost + count)) {
                    let source = if left { ring.l.as_slice() } else { ring.r.as_slice() };
                    copy_from_ring(source, target, from);
                }
            }
            if let Some(tail) = out.get_mut(lost + count..) {
                tail.fill(0.0);
            }
        }
    }
}

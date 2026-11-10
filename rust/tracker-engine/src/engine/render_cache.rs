//! Timestamped bus history of `TrackerEngine.cpp`: `renderCached` (render_bus/render_channel),
//! the per-bus rings (`writeRing`, `zeroRing`, `silenceRing`), retained clock and freeze metadata
//! (`retainClock`, `retainFreeze`) and the queries over them (`prerollClock`,
//! `st_engine_clock_span`, `st_engine_freeze_span`).
//!
//! Serialized graph pulls may overlap and use different frame counts; only an uncached suffix
//! advances the sequencer and DSP, existing ranges are copied without advancing music twice.

use super::abi::*;
use super::timing::fixed_clock_context;
use super::state::*;
use super::tables::*;
use super::util::{cmax, cmin};
use std::sync::atomic::Ordering;

const LIMIT_52: i64 = 1i64 << 52;
const LIMIT_30: i64 = 1i64 << 30;

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

    /// `renderCached`: one bus (or logical channel) of `n` frames at graph time `time`. The
    /// caller has already rejected `n > CACHE_CAPACITY`, a negative bus and timestamp overflow.
    pub fn render_cached(
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
            self.release_clock_origin();
            if !self.freeze_transition_pending || self.freeze_natural_tail {
                self.freeze_transition(ST_FREEZE_CAUSE_GRAPH_RESET, true, false);
            }
            self.timing_epoch = self.timing_epoch.wrapping_add(1);
            self.cache_begin = time;
            self.cache_end = time;
            self.clock_begin = time;
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
            if self.cache_end - self.clock_begin > CLOCK_CAPACITY as u64 {
                self.clock_begin = self.cache_end - CLOCK_CAPACITY as u64;
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

    /// Clock context before the first prepared timestamp: the immutable initial map/consumed
    /// transport origin (PREROLL if playing, no backward loop); a stopped origin holds.
    fn preroll_clock(&self, graph_frame: i64, content: i64, requested: u32, latency: i64, out: &mut STClockSpan) -> i32 {
        let origin_graph = self.cache_origin_graph as i64;
        if !self.cache_origin_valid || content >= origin_graph || origin_graph.wrapping_sub(content) > LIMIT_30 {
            return ST_ENGINE_CLOCK_NOT_RETAINED;
        }
        let origin = &self.cache_origin_clock;
        let active = (origin.flags & ST_CLOCK_PLAYING) != 0;
        let frame = self.cache_origin_transport + (if active { content.wrapping_sub(origin_graph) as f64 } else { 0.0 });
        let mut result = STClockSpan::default();
        let c = &mut result.context;
        if !self.cache_origin_timing.is_null() {
            // SAFETY: the cache origin pins this map (`origin_pins`), so `collect()` keeps it
            // alive until `release_clock_origin`.
            let timing = unsafe { &*self.cache_origin_timing };
            if timing.context(frame as i64, c) != ST_CLOCK_OK {
                return ST_CLOCK_OUTSIDE_DOMAIN;
            }
        } else {
            fixed_clock_context(c, frame, self.sample_rate, origin.bpm, origin.numerator, origin.denominator);
        }
        let song_frame = c.song_frame;
        c.graph_frame = graph_frame;
        c.content_graph_frame = content;
        c.revision = origin.revision;
        c.epoch = origin.epoch;
        c.loop_iteration = 0;
        c.loop_start_beat = origin.loop_start_beat;
        c.loop_end_beat = origin.loop_end_beat;
        c.flags = (c.flags & ST_CLOCK_DOMAIN_END)
            | (origin.flags & (ST_CLOCK_PLAYING | ST_CLOCK_LOOPING | ST_CLOCK_RECORDING))
            | (if active { ST_CLOCK_PREROLL } else { 0 });
        let translated =
            |value: i64| if value == i64::MAX { value } else { graph_frame.wrapping_add(value).wrapping_sub(song_frame) };
        c.next_boundary_frame = if active { translated(c.next_boundary_frame) } else { i64::MAX };
        c.next_quarter_beat_frame = if active { translated(c.next_quarter_beat_frame) } else { i64::MAX };
        c.next_quarter_grid_frame = translated(c.next_quarter_grid_frame);
        let origin_frame = origin_graph.wrapping_add(latency);
        if c.next_boundary_frame > origin_frame {
            c.next_boundary_frame = origin_frame;
            c.next_boundary_kinds = ST_CLOCK_BOUNDARY_ORIGIN;
        } else if c.next_boundary_frame == origin_frame {
            c.next_boundary_kinds |= ST_CLOCK_BOUNDARY_ORIGIN;
        }
        if c.next_quarter_beat_frame >= origin_frame {
            c.next_quarter_beat_frame = if origin.next_quarter_beat_frame == i64::MAX {
                origin.next_quarter_beat_frame
            } else {
                origin.next_quarter_beat_frame.wrapping_add(latency)
            };
        }
        if c.next_quarter_beat_frame == i64::MAX {
            c.flags |= ST_CLOCK_GRID_PROJECTION;
        }
        result.frames = cmin(requested as i64, cmax(0, c.next_boundary_frame.wrapping_sub(graph_frame))) as u32;
        *out = result;
        ST_CLOCK_OK
    }

    /// `st_engine_clock_span`
    pub fn clock_span(&self, sh: &Shared, graph_frame: i64, requested: u32, latency: i64, out: &mut STClockSpan) -> i32 {
        if requested < 1
            || requested > ST_CLOCK_MAX_SPAN
            || latency < 0
            || latency > LIMIT_30
            || graph_frame < -LIMIT_52
            || graph_frame > LIMIT_52
        {
            return ST_CLOCK_INVALID;
        }
        let content = graph_frame - latency;
        if !self.cache_valid || self.observed_cache_epoch != sh.cache_epoch.load(Ordering::Acquire) {
            return ST_ENGINE_CLOCK_NOT_PREPARED;
        }
        if self.cache_origin_valid && content < self.cache_origin_graph as i64 {
            return self.preroll_clock(graph_frame, content, requested, latency, out);
        }
        if content < 0 || (content as u64) < self.clock_begin {
            return ST_ENGINE_CLOCK_NOT_RETAINED;
        }
        let content_frame = content as u64;
        if content_frame >= self.cache_end {
            return ST_ENGINE_CLOCK_NOT_PREPARED;
        }
        let at = |frame: u64| self.cached_clock[(frame % CLOCK_CAPACITY as u64) as usize];
        let mut result = STClockSpan { context: at(content_frame), frames: 0 };
        let c = &mut result.context;
        c.graph_frame = graph_frame;
        c.content_graph_frame = content;
        if c.next_boundary_frame != i64::MAX {
            c.next_boundary_frame = c.next_boundary_frame.wrapping_add(latency);
        }
        if c.next_quarter_beat_frame != i64::MAX {
            c.next_quarter_beat_frame = c.next_quarter_beat_frame.wrapping_add(latency);
        }
        if c.next_quarter_grid_frame != i64::MAX {
            c.next_quarter_grid_frame = c.next_quarter_grid_frame.wrapping_add(latency);
        }
        let mut count =
            cmin(requested as u64, cmin(self.cache_end - content_frame, (LIMIT_52 - graph_frame + 1) as u64)) as u32;
        if c.next_boundary_frame != i64::MAX {
            count = cmin(count as i64, cmax(0, c.next_boundary_frame.wrapping_sub(graph_frame))) as u32;
        }
        let changed = (1..count).find(|&n| {
            let next = at(content_frame + n as u64);
            next.epoch != c.epoch || next.revision != c.revision
        });
        if let Some(n) = changed {
            count = n;
            c.next_boundary_frame = graph_frame + n as i64;
            c.next_boundary_kinds = ST_CLOCK_BOUNDARY_ORIGIN;
        }
        result.frames = count;
        *out = result;
        ST_CLOCK_OK
    }

    /// `st_engine_freeze_span`
    pub fn freeze_span(&self, sh: &Shared, graph_frame: i64, requested: u32, latency: i64, out: &mut STFreezeSpan) -> i32 {
        if requested < 1
            || requested > ST_CLOCK_MAX_SPAN
            || latency < 0
            || latency > LIMIT_30
            || graph_frame < -LIMIT_52
            || graph_frame > LIMIT_52
        {
            return ST_CLOCK_INVALID;
        }
        let history = sh.freeze_history.load(Ordering::Acquire);
        if history.is_null() {
            return ST_FREEZE_NOT_CONFIGURED;
        }
        // SAFETY: the history is allocated once by the control thread and freed only when the
        // engine is destroyed; this query is serialized with render_bus (API contract), the
        // history's only writer.
        let history = unsafe { &*history };
        if !self.cache_valid || self.observed_cache_epoch != sh.cache_epoch.load(Ordering::Acquire) {
            return ST_ENGINE_CLOCK_NOT_PREPARED;
        }
        let content = graph_frame - latency;
        let mut result = STFreezeSpan::default();
        let origin_graph = self.cache_origin_graph as i64;
        if self.cache_origin_valid && content < origin_graph {
            if !self.cache_origin_freeze_valid || origin_graph.wrapping_sub(content) > LIMIT_30 {
                return ST_ENGINE_CLOCK_NOT_RETAINED;
            }
            result.context = self.cache_origin_freeze;
            result.context.phase = ST_FREEZE_STOPPED;
            result.context.flags |= ST_FREEZE_PREROLL;
            result.frames =
                cmin(requested as i64, cmin(origin_graph.wrapping_sub(content), LIMIT_52 - graph_frame + 1)) as u32;
        } else {
            if content < 0 || (content as u64) < self.clock_begin {
                return ST_ENGINE_CLOCK_NOT_RETAINED;
            }
            let content_frame = content as u64;
            if content_frame >= self.cache_end {
                return ST_ENGINE_CLOCK_NOT_PREPARED;
            }
            let at = |frame: u64| history.frames[(frame % CLOCK_CAPACITY as u64) as usize];
            let first = at(content_frame);
            if first.content_graph_frame != content {
                return ST_ENGINE_CLOCK_NOT_RETAINED;
            }
            result.context = first;
            result.frames =
                cmin(requested as u64, cmin(self.cache_end - content_frame, (LIMIT_52 - graph_frame + 1) as u64)) as u32;
            let advancing = first.phase == ST_FREEZE_SONG || first.phase == ST_FREEZE_NATURAL_TAIL;
            for n in 1..result.frames {
                let next = at(content_frame + n as u64);
                if next.content_graph_frame != content + n as i64
                    || next.publication_id != first.publication_id
                    || next.playback_id != first.playback_id
                    || next.transition_id != first.transition_id
                    || next.phase != first.phase
                    || next.cause != first.cause
                    || next.flags != first.flags
                    || next.project_frame != first.project_frame.wrapping_add(if advancing { n as i64 } else { 0 })
                {
                    result.frames = n;
                    break;
                }
            }
        }
        result.context.graph_frame = graph_frame;
        result.context.content_graph_frame = content;
        *out = result;
        ST_CLOCK_OK
    }
}

impl<'a> Rt<'a> {
    fn freeze_position_valid(frame: f64) -> bool {
        frame.is_finite() && frame >= 0.0 && frame <= LIMIT_52 as f64 && frame == frame.floor()
    }

    /// Record the freeze metadata of one graph frame (and of the cache origin).
    fn retain_freeze(&mut self, graph_frame: u64, origin: bool) {
        let history = self.sh.freeze_history.load(Ordering::Acquire);
        if history.is_null() {
            return;
        }
        let p = self.p;
        let e = &mut *self.e;
        let mut c = STFreezeContext {
            publication_id: if p.frozen_song_policy { p.frozen_publication_id } else { 0 },
            playback_id: e.freeze_playback_id,
            transition_id: e.freeze_transition_id,
            graph_frame: graph_frame as i64,
            content_graph_frame: graph_frame as i64,
            sample_rate: e.sample_rate,
            cause: e.freeze_cause,
            ..STFreezeContext::default()
        };
        let valid = Self::freeze_position_valid(e.transport_frames) && e.freeze_playback_id != 0 && e.freeze_transition_id != 0;
        if e.freeze_transition_pending {
            e.freeze_transition_graph = c.content_graph_frame;
            e.freeze_transition_project = if valid { e.transport_frames as i64 } else { 0 };
            e.freeze_transition_pending = false;
        }
        c.transition_content_graph_frame = e.freeze_transition_graph;
        c.transition_project_frame = e.freeze_transition_project;
        c.project_frame = if valid { e.transport_frames as i64 } else { 0 };
        if !p.frozen_song_policy {
            c.phase = ST_FREEZE_DISABLED;
        } else if !e.song_mode {
            c.phase = ST_FREEZE_UNSUPPORTED;
        } else if !valid {
            c.phase = ST_FREEZE_INVALID_POSITION;
        } else if e.playing {
            c.phase = ST_FREEZE_SONG;
        } else if e.freeze_natural_tail
            && e.freeze_cause == ST_FREEZE_CAUSE_NATURAL_END
            && e.freeze_transition_graph != i64::MIN
        {
            let elapsed = c.content_graph_frame.wrapping_sub(e.freeze_transition_graph);
            if elapsed < 0 || elapsed > LIMIT_52 - e.freeze_transition_project {
                c.phase = ST_FREEZE_INVALID_POSITION;
            } else {
                c.phase = ST_FREEZE_NATURAL_TAIL;
                c.project_frame = e.freeze_transition_project + elapsed;
            }
        } else {
            c.phase = ST_FREEZE_STOPPED;
        }
        // SAFETY: the history is allocated by the control thread before the policy that uses it
        // is published and lives until the engine is destroyed; the render thread is its only
        // writer, and readers (`freeze_span`) are serialized with rendering.
        let history = unsafe { &mut *history };
        if let Some(slot) = history.frames.get_mut((graph_frame % CLOCK_CAPACITY as u64) as usize) {
            *slot = c;
        }
        if origin {
            e.cache_origin_freeze = c;
            e.cache_origin_freeze_valid = true;
        }
    }

    /// Record the clock context of one newly rendered graph frame.
    pub fn retain_clock(&mut self, graph_frame: u64) {
        let slot = (graph_frame % CLOCK_CAPACITY as u64) as usize;
        if graph_frame > (1u64 << 52) {
            if let Some(entry) = self.e.cached_clock.get_mut(slot) {
                *entry = STClockContext::default();
            }
            return;
        }
        let p = self.p;
        let mut c = self.e.musical_context;
        c.graph_frame = graph_frame as i64;
        c.content_graph_frame = c.graph_frame;
        let transport_frames = self.e.transport_frames;
        let song_frame = transport_frames as i64;
        c.song_frame = song_frame;
        let graph = c.graph_frame;
        let translate = |frame: i64| if frame == i64::MAX { frame } else { graph.wrapping_add(frame).wrapping_sub(song_frame) };
        c.next_boundary_frame = translate(c.next_boundary_frame);
        c.next_quarter_beat_frame = translate(c.next_quarter_beat_frame);
        c.next_quarter_grid_frame = translate(c.next_quarter_grid_frame);
        if self.e.playing && (self.e.song_mode || p.timing.is_some()) {
            // A section loops before the song endpoint, except when playback started beyond the
            // section and must first finish the outro.
            let section = self.song_loop_active();
            let in_section = section && self.e.sequence <= self.song_loop_last();
            let endpoint =
                if in_section { self.occurrence_start_frames(self.song_loop_last() + 1) } else { self.e.song_end_frames };
            let end: i64 = if self.e.song_mode {
                endpoint.ceil() as i64
            } else {
                match &p.timing {
                    Some(timing) => timing.frame(self.e.pattern_origin_beat + self.pattern_domain_end_beat()),
                    None => 0,
                }
            };
            let until: i64 =
                if p.timing.is_some() { end.wrapping_sub(song_frame) } else { (endpoint - transport_frames).ceil() as i64 };
            let boundary = graph.wrapping_add(cmax(1, until));
            let kind = if (c.flags & ST_CLOCK_LOOPING) != 0 { ST_CLOCK_BOUNDARY_LOOP } else { ST_CLOCK_BOUNDARY_DOMAIN };
            if boundary < c.next_boundary_frame {
                c.next_boundary_frame = boundary;
                c.next_boundary_kinds = kind;
            } else if boundary == c.next_boundary_frame {
                c.next_boundary_kinds |= kind;
            }
            if c.next_quarter_beat_frame >= boundary {
                c.next_quarter_beat_frame = i64::MAX;
                if (c.flags & ST_CLOCK_LOOPING) != 0 {
                    let first = c.loop_start_beat.ceil();
                    if first < c.loop_end_beat {
                        let rate = self.e.sample_rate;
                        let (lo, next) = match &p.timing {
                            Some(timing) => (timing.frame(c.loop_start_beat), timing.frame(first)),
                            None => (0, (first * 60.0 / p.bpm * rate).ceil() as i64),
                        };
                        if next < end {
                            if section && p.timing.is_none() {
                                let start = self.occurrence_start_frames(p.song_loop_start);
                                let overshoot = if self.e.sequence <= self.song_loop_last() {
                                    transport_frames + until as f64 - endpoint
                                } else {
                                    0.0
                                };
                                let target = first * 60.0 / p.bpm * rate;
                                c.next_quarter_beat_frame =
                                    boundary.wrapping_add(cmax(0.0, (target - start - overshoot).ceil()) as i64);
                            } else {
                                c.next_quarter_beat_frame = boundary.wrapping_add(next).wrapping_sub(lo);
                            }
                        }
                    }
                }
            }
        }
        if c.next_quarter_beat_frame == i64::MAX {
            c.flags |= ST_CLOCK_GRID_PROJECTION;
        } else {
            c.flags &= !ST_CLOCK_GRID_PROJECTION;
        }
        let origin = !self.e.cache_origin_valid;
        self.retain_freeze(graph_frame, origin);
        let e = &mut *self.e;
        if origin {
            e.cache_origin_clock = c;
            e.cache_origin_transport = transport_frames;
            e.cache_origin_graph = graph_frame;
            e.cache_origin_timing = match &p.timing {
                Some(timing) => {
                    timing.origin_pins.fetch_add(1, Ordering::Relaxed);
                    &**timing as *const _
                }
                None => std::ptr::null(),
            };
            e.cache_origin_valid = true;
        }
        if let Some(entry) = e.cached_clock.get_mut(slot) {
            *entry = c;
        }
    }
}

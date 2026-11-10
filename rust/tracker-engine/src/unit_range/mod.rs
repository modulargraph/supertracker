//! Bounded render cache for one hosted unit.
//!
//! A [`UnitRangeCache`] retains the most recent output of one child processor (a plug-in) in a
//! ring, so overlapping or repeated graph ranges are served from memory and only the uncached
//! suffix is rendered, in spans the clock dictates. The child is a [`UnitRangeSource`].
//!
//! # Threading and real-time properties
//!
//! * [`UnitRangeCache::new`] and dropping the cache are control-thread operations and the only
//!   places that allocate or free. `render`, `reset` and `stats` never allocate, lock or wait.
//! * One atomic flag admits a single caller at a time. A concurrent or reentrant `render`,
//!   `reset` or `stats` returns [`UnitRangeError::Busy`] immediately instead of waiting.
//! * Cache publication is transactional: new PCM is rendered into scratch storage and committed
//!   to the ring only after every child call succeeded. A child that fails may already have
//!   advanced, so its error is latched until `reset`.
//!
//! # How the shared state is kept sound
//!
//! A cache is used through `&self` from any thread (it is `Sync`), and a source may call back
//! into the cache it is rendering for. The configuration fields are immutable after creation and
//! may be read by anyone (the busy path needs them to clear the output). Everything mutable
//! lives in one `UnsafeCell<State>` that is only reachable through a private guard, and a guard
//! exists only while its holder owns the `rendering` flag.

use std::cell::UnsafeCell;
use std::fmt;
use std::num::NonZeroI32;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "clock")]
use crate::clock as clock_types;
// The cache needs only the clock's data types, so it also builds without the clock itself.
#[cfg(not(feature = "clock"))]
#[path = "../clock/types.rs"]
#[allow(dead_code)]
mod clock_types;

use clock_types::{ClockContext, ClockQueryError, ClockSpan};

#[cfg(test)]
mod tests;

/// Why a request to a [`UnitRangeCache`] failed.
///
/// Every error has a numeric [`code`](UnitRangeError::code), which is what
/// [`UnitRangeStats::last_error`] and [`UnitRangeStats::latched_error`] hold. Cache errors are
/// separate from the status values of the source: a failed `prepare` or `process` result is
/// passed through verbatim as [`UnitRangeError::Source`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitRangeError {
    /// No frames, too few or too short output buffers, or an uncached range without a source.
    Invalid,
    /// More frames than the cache's request capacity.
    Oversize,
    /// The range runs past the last graph frame.
    Overflow,
    /// The range starts after the rendered end: the frames between were never rendered.
    Gap,
    /// The range starts before the retained history.
    Expired,
    /// Another call on this cache is in progress (on any thread, or further up this stack).
    Busy,
    /// The source's clock refused a span.
    ClockError,
    /// The clock's domain ends before the request does.
    DomainEnd,
    /// A span the source returned does not fit the request or the cached format.
    ContextMismatch,
    /// The request needs more child calls than the cache was created for.
    SpanLimit,
    /// The source's own failure. A failed `process` stays latched until `reset`: no later
    /// request may retry a partially advanced processor.
    Source(NonZeroI32),
}

impl UnitRangeError {
    /// The numeric code of this error; 0 is success.
    pub const fn code(self) -> i32 {
        match self {
            UnitRangeError::Invalid => -74000,
            UnitRangeError::Oversize => -74001,
            UnitRangeError::Overflow => -74002,
            UnitRangeError::Gap => -74003,
            UnitRangeError::Expired => -74004,
            UnitRangeError::Busy => -74005,
            UnitRangeError::ClockError => -74006,
            UnitRangeError::DomainEnd => -74007,
            UnitRangeError::ContextMismatch => -74008,
            UnitRangeError::SpanLimit => -74009,
            UnitRangeError::Source(code) => code.get(),
        }
    }

    /// The error with this code; `None` for 0. A code outside the cache's own range is a
    /// source failure.
    pub const fn from_code(code: i32) -> Option<UnitRangeError> {
        Some(match code {
            -74000 => UnitRangeError::Invalid,
            -74001 => UnitRangeError::Oversize,
            -74002 => UnitRangeError::Overflow,
            -74003 => UnitRangeError::Gap,
            -74004 => UnitRangeError::Expired,
            -74005 => UnitRangeError::Busy,
            -74006 => UnitRangeError::ClockError,
            -74007 => UnitRangeError::DomainEnd,
            -74008 => UnitRangeError::ContextMismatch,
            -74009 => UnitRangeError::SpanLimit,
            _ => match NonZeroI32::new(code) {
                Some(code) => UnitRangeError::Source(code),
                None => return None,
            },
        })
    }
}

impl fmt::Display for UnitRangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnitRangeError::Invalid => formatter.write_str("The unit render request is invalid."),
            UnitRangeError::Oversize => formatter.write_str("The unit render request exceeds the cache's request capacity."),
            UnitRangeError::Overflow => formatter.write_str("The unit render request runs past the last graph frame."),
            UnitRangeError::Gap => formatter.write_str("The unit render request leaves a gap after the rendered range."),
            UnitRangeError::Expired => formatter.write_str("The unit render request starts before the retained history."),
            UnitRangeError::Busy => formatter.write_str("The unit cache is in use."),
            UnitRangeError::ClockError => formatter.write_str("The clock refused a span of the unit render request."),
            UnitRangeError::DomainEnd => formatter.write_str("The clock's domain ends inside the unit render request."),
            UnitRangeError::ContextMismatch => formatter.write_str("A clock span does not fit the unit render request."),
            UnitRangeError::SpanLimit => formatter.write_str("The unit render request needs more spans than the cache allows."),
            UnitRangeError::Source(code) => write!(formatter, "The unit failed with status {code}."),
        }
    }
}

impl std::error::Error for UnitRangeError {}

/// Why [`UnitRangeCache::new`] refused a configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitRangeConfigError(&'static str);

impl UnitRangeConfigError {
    pub const fn message(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for UnitRangeConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for UnitRangeConfigError {}

/// The child of a [`UnitRangeCache`]: what produces the audio the cache retains.
///
/// All three methods run synchronously on the thread that called [`UnitRangeCache::render`] and
/// must not allocate, wait or use the cache (a call back into it is refused as
/// [`UnitRangeError::Busy`]). Only the uncached suffix of a request is prepared and processed.
/// The complete span plan is made before the first `process` call or per-child event delivery.
/// A request served from retained audio calls none of them.
pub trait UnitRangeSource {
    /// Extend the upstream audio, clock and event caches for `frames` frames at `graph_frame`,
    /// without sending events to the child or advancing it. An error ends the request and may be
    /// retried. The default does nothing.
    fn prepare(&mut self, graph_frame: i64, frames: u32) -> Result<(), NonZeroI32> {
        let _ = (graph_frame, frames);
        Ok(())
    }

    /// The clock span at `graph_frame`, at most `frames` long (at most the cache's child frame
    /// limit is asked for). [`ClockQueryError::OutsideDomain`] ends the request as
    /// [`UnitRangeError::DomainEnd`], any other error as [`UnitRangeError::ClockError`].
    fn span(&mut self, graph_frame: i64, frames: u32) -> Result<ClockSpan, ClockQueryError>;

    /// Render `frames` frames at `graph_frame` into `output`: one buffer per channel, each
    /// `frames` long and zeroed. `context` is the span's. An error is latched: the child may
    /// have advanced, so the cache refuses further requests until it is reset.
    fn process(&mut self, context: &ClockContext, graph_frame: i64, frames: u32, output: &mut [&mut [f32]]) -> Result<(), NonZeroI32>;
}

/// A snapshot of a cache's counters and of what it retains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnitRangeStats {
    pub requests: u64,
    pub successful_requests: u64,
    pub cache_hits: u64,
    pub prepared_frames: u64,
    pub processed_frames: u64,
    pub process_calls: u64,
    pub retained_start_frame: i64,
    pub rendered_end_frame: i64,
    pub revision: u64,
    pub epoch: u64,
    pub retained_frames: u32,
    /// 1 once a first request has succeeded and fixed the graph position of the history.
    pub anchored: u32,
    /// [`UnitRangeError::code`] of the last request; 0 when it succeeded.
    pub last_error: i32,
    /// [`UnitRangeError::code`] of the child failure that blocks the cache until `reset`; 0 none.
    pub latched_error: i32,
}

/// The render cache of one hosted unit. See the module documentation.
pub struct UnitRangeCache {
    // Immutable after creation; readable without owning `rendering`.
    channels: u32,
    maximum_request: u32,
    history_frames: u32,
    maximum_child: u32,
    rendering: AtomicBool,
    state: UnsafeCell<State>,
}

/// Everything a render mutates. Only reachable through a [`RenderGuard`].
struct State {
    write_position: u32,
    content_offset: i64,
    sample_rate: f64,
    stats: UnitRangeStats,
    /// Ring of retained PCM: `channels` consecutive blocks of `history_frames` samples.
    history: Vec<f32>,
    /// The suffix being rendered: `channels` consecutive blocks of `maximum_request` samples.
    scratch: Vec<f32>,
    /// The preflighted child calls of one request: exactly `maximum_spans` entries.
    plan: Vec<ClockSpan>,
}

// SAFETY: any thread may call into a cache through `&self`. The configuration fields never
// change, `rendering` is atomic, and `state` is only accessed by the one caller that owns
// `rendering` (see `RenderGuard`), so shared references on several threads never race.
unsafe impl Sync for UnitRangeCache {}

/// Ownership of a cache's `rendering` flag, and with it of the mutable state.
struct RenderGuard<'a> {
    cache: &'a UnitRangeCache,
}

impl<'a> RenderGuard<'a> {
    /// `None` when another call (on any thread, or further up this thread's stack) is active.
    fn acquire(cache: &'a UnitRangeCache) -> Option<Self> {
        if cache.rendering.swap(true, Ordering::Acquire) {
            None
        } else {
            Some(Self { cache })
        }
    }

    fn state(&mut self) -> &mut State {
        // SAFETY: at most one guard exists per cache at a time: `acquire` hands one out only on
        // the false -> true transition of `rendering`, and the flag is cleared only when that
        // guard is dropped. The returned borrow is tied to `&mut self`, so this guard cannot
        // produce a second reference either. Nothing else touches `state`: a reentrant call
        // from a source only forms a shared reference to the cache and fails `acquire` before
        // it looks inside the cell.
        unsafe { &mut *self.cache.state.get() }
    }
}

impl Drop for RenderGuard<'_> {
    fn drop(&mut self) {
        self.cache.rendering.store(false, Ordering::Release);
    }
}

/// Copies `frames` retained samples starting at graph frame `start` to the caller's output.
/// `[start, start + frames)` lies within the retained range, `frames` is at most
/// `maximum_request`, and `output` holds one buffer of at least `frames` samples per channel.
fn copy_cached(cache: &UnitRangeCache, state: &State, start: i64, frames: u32, output: &mut [&mut [f32]]) {
    let history_frames = cache.history_frames;
    let offset = (start as u64).wrapping_sub(state.stats.retained_start_frame as u64);
    let oldest = (state.write_position + history_frames - state.stats.retained_frames) % history_frames;
    let position = (u64::from(oldest).wrapping_add(offset) % u64::from(history_frames)) as u32;
    let first = frames.min(history_frames - position) as usize;
    let (position, frames) = (position as usize, frames as usize);
    let channels = state.history.chunks_exact(history_frames as usize).take(cache.channels as usize);
    for (source, destination) in channels.zip(output.iter_mut()) {
        // `position < history_frames` and `first <= history_frames - position`, and the wrapped
        // remainder `frames - first` is at most `maximum_request <= history_frames`, so both
        // source ranges lie inside this channel's block.
        destination[..first].copy_from_slice(&source[position..position + first]);
        if first < frames {
            destination[first..frames].copy_from_slice(&source[..frames - first]);
        }
    }
}

/// Appends the first `frames` scratch samples of each channel to the ring; `end` is the graph
/// frame after them and `last` the context of the final span.
fn commit(
    cache: &UnitRangeCache,
    state: &mut State,
    end: i64,
    frames: u32,
    last: &ClockContext,
    sample_rate: f64,
    content_offset: i64,
) {
    let history_frames = cache.history_frames;
    let position = state.write_position as usize;
    let first = frames.min(history_frames - state.write_position) as usize;
    let count = frames as usize;
    let destinations = state.history.chunks_exact_mut(history_frames as usize);
    let sources = state.scratch.chunks_exact(cache.maximum_request as usize);
    // `frames <= maximum_request <= history_frames` and `write_position < history_frames`.
    for (destination, source) in destinations.zip(sources) {
        destination[position..position + first].copy_from_slice(&source[..first]);
        if first < count {
            destination[..count - first].copy_from_slice(&source[first..count]);
        }
    }
    state.write_position = (state.write_position + frames) % history_frames;
    state.stats.retained_frames = history_frames.min(state.stats.retained_frames + frames);
    state.stats.rendered_end_frame = end;
    // Retention can never precede the first successfully anchored graph frame, so
    // this subtraction cannot underflow even for an anchor near INT64_MIN.
    state.stats.retained_start_frame = end - i64::from(state.stats.retained_frames);
    state.stats.anchored = 1;
    state.stats.revision = last.revision;
    state.stats.epoch = last.epoch;
    state.sample_rate = sample_rate;
    state.content_offset = content_offset;
}

/// `count` default values, or `None` if the memory cannot be had.
fn try_filled<T: Clone + Default>(count: usize) -> Option<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(count).ok()?;
    values.resize(count, T::default());
    Some(values)
}

impl UnitRangeCache {
    /// Control-thread allocation. `channels` 1 or 2, `maximum_request_frames` 1..=65536,
    /// `history_frames` from the request capacity up to 131072, `maximum_child_frames` 1 up to
    /// the request capacity, `maximum_spans` 1..=256. `maximum_spans` counts actual child calls,
    /// including the splitting at `maximum_child_frames`, and must cover
    /// `ceil(maximum_request_frames / maximum_child_frames)`.
    pub fn new(
        channels: u32,
        maximum_request_frames: u32,
        history_frames: u32,
        maximum_child_frames: u32,
        maximum_spans: u32,
    ) -> Result<UnitRangeCache, UnitRangeConfigError> {
        let refuse = |message| Err(UnitRangeConfigError(message));
        if !(1..=2).contains(&channels) {
            return refuse("Unit cache requires one or two channels.");
        }
        if !(1..=65536).contains(&maximum_request_frames) {
            return refuse("Unit request capacity must be 1..65536 frames.");
        }
        if history_frames < maximum_request_frames || history_frames > 131072 {
            return refuse("Unit history must cover one maximum request and not exceed 131072 frames.");
        }
        if maximum_child_frames < 1 || maximum_child_frames > maximum_request_frames {
            return refuse("Child frame limit must fit the request capacity.");
        }
        if !(1..=256).contains(&maximum_spans) {
            return refuse("Unit span capacity must be 1..256.");
        }
        if maximum_spans < maximum_request_frames.div_ceil(maximum_child_frames) {
            return refuse("Unit span capacity cannot cover the maximum request at the child frame limit.");
        }
        const OUT_OF_MEMORY: UnitRangeConfigError = UnitRangeConfigError("Could not allocate bounded unit render storage.");
        let channel_count = channels as usize;
        let state = State {
            write_position: 0,
            content_offset: 0,
            sample_rate: 0.0,
            stats: UnitRangeStats::default(),
            history: try_filled(channel_count * history_frames as usize).ok_or(OUT_OF_MEMORY)?,
            scratch: try_filled(channel_count * maximum_request_frames as usize).ok_or(OUT_OF_MEMORY)?,
            plan: try_filled(maximum_spans as usize).ok_or(OUT_OF_MEMORY)?,
        };
        Ok(UnitRangeCache {
            channels,
            maximum_request: maximum_request_frames,
            history_frames,
            maximum_child: maximum_child_frames,
            rendering: AtomicBool::new(false),
            state: UnsafeCell::new(state),
        })
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// The most frames one request may ask for.
    pub fn maximum_request_frames(&self) -> u32 {
        self.maximum_request
    }

    /// Forgets all retained audio, the anchor and any latched error, and clears the statistics.
    /// [`UnitRangeError::Busy`] while another call is in progress.
    pub fn reset(&self) -> Result<(), UnitRangeError> {
        let Some(mut guard) = RenderGuard::acquire(self) else {
            return Err(UnitRangeError::Busy);
        };
        let state = guard.state();
        state.stats = UnitRangeStats::default();
        state.write_position = 0;
        state.content_offset = 0;
        state.sample_rate = 0.0;
        Ok(())
    }

    /// Bounded snapshot; [`UnitRangeError::Busy`] while another call is in progress.
    pub fn stats(&self) -> Result<UnitRangeStats, UnitRangeError> {
        let Some(mut guard) = RenderGuard::acquire(self) else {
            return Err(UnitRangeError::Busy);
        };
        Ok(guard.state().stats)
    }

    /// Clears the caller's output: at most `maximum_request` samples per channel, which keeps
    /// rejection work bounded. Uses only immutable configuration, so it is fine while busy.
    fn silence(&self, frames: u32, output: &mut [&mut [f32]]) {
        let bounded = frames.min(self.maximum_request) as usize;
        for channel in output.iter_mut().take(self.channels as usize) {
            let count = bounded.min(channel.len());
            channel[..count].fill(0.0);
        }
    }

    /// Renders `frames` frames at `graph_frame` into `output`: one buffer per channel, each at
    /// least `frames` long (further buffers are ignored, further samples left alone).
    ///
    /// The first success anchors any clock-valid graph frame. Further requests may repeat
    /// retained ranges, or overlap the retained prefix and append a contiguous suffix. Gaps,
    /// expired history, bad spans, a changed content offset or sample rate, and arithmetic
    /// overflow fail without changing good cached PCM. A request that lies entirely in the
    /// retained range needs no `source` and calls none, also after the caller replaced its
    /// clock; any other request without a source is [`UnitRangeError::Invalid`].
    /// Revision/epoch changes at span boundaries are forwarded to the child, including within
    /// one request; they do not invalidate old PCM. Changing the sample rate or the upstream
    /// content-frame offset requires a quiescent reset. Spans must stay within their context's
    /// `next_boundary_frame` and carry finite beat/bar/tempo values, a positive tempo and
    /// numerator, and denominator 2/4/8/16.
    ///
    /// Failures clear the output; an oversized call clears only `maximum_request_frames`
    /// samples per channel, keeping rejection work bounded. A concurrent or reentrant render is
    /// rejected immediately. No render-time allocation, locks or waiting.
    pub fn render(
        &self,
        graph_frame: i64,
        frames: u32,
        output: &mut [&mut [f32]],
        source: Option<&mut dyn UnitRangeSource>,
    ) -> Result<(), UnitRangeError> {
        let Some(mut guard) = RenderGuard::acquire(self) else {
            self.silence(frames, output);
            return Err(UnitRangeError::Busy);
        };
        let state = guard.state();
        // Every rejection records the error and clears the caller's output.
        macro_rules! fail {
            ($error:expr) => {{
                let error: UnitRangeError = $error;
                state.stats.last_error = error.code();
                self.silence(frames, output);
                Err(error)
            }};
        }
        state.stats.requests = state.stats.requests.wrapping_add(1);
        let channels = self.channels as usize;
        let needed = frames.min(self.maximum_request) as usize;
        let valid_output = output.len() >= channels && output.iter().take(channels).all(|channel| channel.len() >= needed);
        if frames == 0 || !valid_output {
            return fail!(UnitRangeError::Invalid);
        }
        if frames > self.maximum_request {
            return fail!(UnitRangeError::Oversize);
        }
        if graph_frame > i64::MAX - i64::from(frames) {
            return fail!(UnitRangeError::Overflow);
        }
        if let Some(latched) = UnitRangeError::from_code(state.stats.latched_error) {
            return fail!(latched);
        }
        let end = graph_frame + i64::from(frames);
        let mut suffix_start = graph_frame;
        if state.stats.anchored != 0 {
            if graph_frame < state.stats.retained_start_frame {
                return fail!(UnitRangeError::Expired);
            }
            if graph_frame > state.stats.rendered_end_frame {
                return fail!(UnitRangeError::Gap);
            }
            if end <= state.stats.rendered_end_frame {
                copy_cached(self, state, graph_frame, frames, output);
                state.stats.cache_hits = state.stats.cache_hits.wrapping_add(1);
                state.stats.successful_requests = state.stats.successful_requests.wrapping_add(1);
                state.stats.last_error = 0;
                return Ok(());
            }
            suffix_start = state.stats.rendered_end_frame;
        }
        let Some(source) = source else {
            return fail!(UnitRangeError::Invalid);
        };
        // graph_frame <= suffix_start < end, so this is 1..=frames.
        let suffix_frames = (end - suffix_start) as u32;

        // The native provider must first retain the new audio/clock/event range. This
        // is deliberately separate from per-child event dispatch in process below.
        if let Err(status) = source.prepare(suffix_start, suffix_frames) {
            return fail!(UnitRangeError::Source(status));
        }
        state.stats.prepared_frames = state.stats.prepared_frames.wrapping_add(u64::from(suffix_frames));

        let mut span_count: usize = 0;
        let mut planned: u32 = 0;
        let mut sample_rate = state.sample_rate;
        let mut content_offset = state.content_offset;
        let mut have_format = state.stats.anchored != 0;
        while planned < suffix_frames {
            // The plan holds exactly maximum_spans entries.
            if span_count >= state.plan.len() {
                return fail!(UnitRangeError::SpanLimit);
            }
            let cursor = suffix_start + i64::from(planned);
            let requested = (suffix_frames - planned).min(self.maximum_child);
            let part = match source.span(cursor, requested) {
                Ok(part) => part,
                Err(ClockQueryError::OutsideDomain) => return fail!(UnitRangeError::DomainEnd),
                Err(ClockQueryError::Invalid) => return fail!(UnitRangeError::ClockError),
            };
            if part.frames == 0 {
                return fail!(UnitRangeError::DomainEnd);
            }
            if part.frames > requested {
                return fail!(UnitRangeError::ContextMismatch);
            }
            let context = &part.context;
            // cursor + part.frames <= end, which was checked against overflow above.
            if context.graph_frame != cursor
                || !context.sample_rate.is_finite()
                || context.sample_rate <= 0.0
                || context.content_graph_frame > context.graph_frame
                || context.next_boundary_frame < cursor + i64::from(part.frames)
                || !context.bpm.is_finite()
                || context.bpm <= 0.0
                || !context.beat.is_finite()
                || !context.bar_start_beat.is_finite()
                || !context.bar_end_beat.is_finite()
                || context.numerator <= 0
                || !matches!(context.denominator, 2 | 4 | 8 | 16)
            {
                return fail!(UnitRangeError::ContextMismatch);
            }
            let offset = (context.graph_frame as u64).wrapping_sub(context.content_graph_frame as u64);
            if offset > 1 << 30 {
                return fail!(UnitRangeError::ContextMismatch);
            }
            if !have_format {
                sample_rate = context.sample_rate;
                content_offset = offset as i64;
                have_format = true;
            } else if context.sample_rate != sample_rate || offset as i64 != content_offset {
                return fail!(UnitRangeError::ContextMismatch);
            }
            state.plan[span_count] = part;
            planned += part.frames;
            span_count += 1;
        }

        // Cache publication is transactional. A child may already have advanced when
        // it reports failure, so its error is latched; scratch PCM is never committed.
        let maximum_request = self.maximum_request as usize;
        for channel in state.scratch.chunks_exact_mut(maximum_request).take(channels) {
            // suffix_frames <= frames <= maximum_request.
            channel[..suffix_frames as usize].fill(0.0);
        }
        let mut processed: usize = 0;
        for index in 0..span_count {
            let part = state.plan[index];
            let count = part.frames as usize;
            state.stats.process_calls = state.stats.process_calls.wrapping_add(1);
            // The plan's spans add up to suffix_frames <= maximum_request, so
            // `processed..processed + count` stays within each channel's block.
            let mut blocks = state.scratch.chunks_exact_mut(maximum_request);
            let mut buffers: [&mut [f32]; 2] = [&mut [], &mut []];
            for buffer in buffers.iter_mut().take(channels) {
                if let Some(block) = blocks.next() {
                    *buffer = &mut block[processed..processed + count];
                }
            }
            if let Err(status) = source.process(&part.context, part.context.graph_frame, part.frames, &mut buffers[..channels]) {
                state.stats.latched_error = status.get();
                return fail!(UnitRangeError::Source(status));
            }
            state.stats.processed_frames = state.stats.processed_frames.wrapping_add(u64::from(part.frames));
            processed += count;
        }
        // suffix_frames >= 1, so at least one span was planned.
        let last = state.plan.get(span_count.saturating_sub(1)).map_or_else(ClockContext::default, |part| part.context);
        commit(self, state, end, suffix_frames, &last, sample_rate, content_offset);
        // After the commit the request lies within the retained range.
        copy_cached(self, state, graph_frame, frames, output);
        state.stats.successful_requests = state.stats.successful_requests.wrapping_add(1);
        state.stats.last_error = 0;
        Ok(())
    }
}

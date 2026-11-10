//! Bounded render cache for one hosted unit: the C API of
//! `Sources/TrackerEngine/include/TrackerUnitRange.h`.
//!
//! Replaces `Sources/TrackerEngine/TrackerUnitRange.cpp`. A cache retains the most recent output
//! of one child processor (a plug-in) in a ring, so overlapping or repeated graph ranges are
//! served from memory and only the uncached suffix is rendered, in spans the clock dictates.
//!
//! # Threading and real-time properties (unchanged from the C++)
//!
//! * `st_unit_range_create` / `destroy` are control-thread operations and the only places that
//!   allocate or free. `render`, `reset` and `get_stats` never allocate, lock or wait.
//! * One atomic flag admits a single caller at a time. A concurrent or reentrant `render`,
//!   `reset` or `get_stats` returns `ST_UNIT_RANGE_BUSY` immediately instead of waiting.
//! * Cache publication is transactional: new PCM is rendered into scratch storage and committed
//!   to the ring only after every child call succeeded. A child that fails may already have
//!   advanced, so its error is latched until `reset`.
//!
//! # How the shared state is kept sound
//!
//! C callers hold a `*mut STUnitRangeCache` and may call from any thread, so every entry point
//! only ever forms a shared reference to the cache. The configuration fields are immutable after
//! creation and may be read by anyone (the BUSY path needs them to clear the output). Everything
//! mutable lives in one `UnsafeCell<State>` that is only reachable through [`RenderGuard`], and a
//! guard exists only while its holder owns the `rendering` flag.

#![deny(unsafe_op_in_unsafe_fn)]

use std::alloc::{alloc, Layout};
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "clock")]
use crate::clock as clock_abi;
// The cache needs only the clock's data types, so it also builds without the clock itself.
#[cfg(not(feature = "clock"))]
#[path = "../clock/abi.rs"]
#[allow(dead_code)]
mod clock_abi;

use clock_abi::{STClockContext, STClockSpan, ST_CLOCK_OK, ST_CLOCK_OUTSIDE_DOMAIN};

#[cfg(test)]
mod tests;

// Cache-specific errors are separate from callback OSStatus values. A nonzero prepare/process
// result is returned verbatim. A failed process result remains latched until reset; no later
// request may retry a partially advanced processor.
pub const ST_UNIT_RANGE_OK: i32 = 0;
pub const ST_UNIT_RANGE_INVALID: i32 = -74000;
pub const ST_UNIT_RANGE_OVERSIZE: i32 = -74001;
pub const ST_UNIT_RANGE_OVERFLOW: i32 = -74002;
pub const ST_UNIT_RANGE_GAP: i32 = -74003;
pub const ST_UNIT_RANGE_EXPIRED: i32 = -74004;
pub const ST_UNIT_RANGE_BUSY: i32 = -74005;
pub const ST_UNIT_RANGE_CLOCK_ERROR: i32 = -74006;
pub const ST_UNIT_RANGE_DOMAIN_END: i32 = -74007;
pub const ST_UNIT_RANGE_CONTEXT_MISMATCH: i32 = -74008;
pub const ST_UNIT_RANGE_SPAN_LIMIT: i32 = -74009;

// All callbacks execute synchronously on the calling render thread and must not throw, allocate,
// wait or mutate the cache. prepare is optional; span/process are required. Only the uncached
// suffix is prepared/processed. prepare extends the native audio/context/event caches first, but
// must not send events to or advance this child. Complete span preflight then finishes before any
// process call or per-child event delivery. Each query requests at most maximumChildFrames.
// Cached-only calls need none of these callbacks.

/// `STUnitRangePrepareFn`
pub type STUnitRangePrepareFn =
    Option<unsafe extern "C" fn(user: *mut c_void, graph_frame: i64, frames: u32) -> i32>;
/// `STUnitRangeSpanFn`
pub type STUnitRangeSpanFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        graph_frame: i64,
        frames: u32,
        span: *mut STClockSpan,
    ) -> i32,
>;
/// `STUnitRangeProcessFn`
pub type STUnitRangeProcessFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        context: *const STClockContext,
        graph_frame: i64,
        frames: u32,
        output: *const *mut f32,
    ) -> i32,
>;

/// `STUnitRangeStats`. Field names are the header's in snake case.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct STUnitRangeStats {
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
    pub anchored: u32,
    pub last_error: i32,
    pub latched_error: i32,
}

// Layout of the struct in TrackerUnitRange.h, as printed by a C++ program using sizeof/offsetof.
const _: () = {
    use std::mem::{align_of, offset_of, size_of};
    assert!(size_of::<STUnitRangeStats>() == 96 && align_of::<STUnitRangeStats>() == 8);
    assert!(offset_of!(STUnitRangeStats, requests) == 0);
    assert!(offset_of!(STUnitRangeStats, successful_requests) == 8);
    assert!(offset_of!(STUnitRangeStats, cache_hits) == 16);
    assert!(offset_of!(STUnitRangeStats, prepared_frames) == 24);
    assert!(offset_of!(STUnitRangeStats, processed_frames) == 32);
    assert!(offset_of!(STUnitRangeStats, process_calls) == 40);
    assert!(offset_of!(STUnitRangeStats, retained_start_frame) == 48);
    assert!(offset_of!(STUnitRangeStats, rendered_end_frame) == 56);
    assert!(offset_of!(STUnitRangeStats, revision) == 64);
    assert!(offset_of!(STUnitRangeStats, epoch) == 72);
    assert!(offset_of!(STUnitRangeStats, retained_frames) == 80);
    assert!(offset_of!(STUnitRangeStats, anchored) == 84);
    assert!(offset_of!(STUnitRangeStats, last_error) == 88);
    assert!(offset_of!(STUnitRangeStats, latched_error) == 92);
};

/// `STUnitRangeCache`. Opaque to C.
pub struct STUnitRangeCache {
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
    stats: STUnitRangeStats,
    /// Ring of retained PCM: `channels` consecutive blocks of `history_frames` samples.
    history: Vec<f32>,
    /// The suffix being rendered: `channels` consecutive blocks of `maximum_request` samples.
    scratch: Vec<f32>,
    /// The preflighted child calls of one request: exactly `maximumSpans` entries.
    plan: Vec<STClockSpan>,
}

// SAFETY: the C API lets any thread call into a cache. The configuration fields never change,
// `rendering` is atomic, and `state` is only accessed by the one caller that owns `rendering`
// (see `RenderGuard`), so shared references on several threads never race.
unsafe impl Sync for STUnitRangeCache {}

/// Ownership of a cache's `rendering` flag, and with it of the mutable state.
struct RenderGuard<'a> {
    cache: &'a STUnitRangeCache,
}

impl<'a> RenderGuard<'a> {
    /// `None` when another call (on any thread, or further up this thread's stack) is active.
    fn acquire(cache: &'a STUnitRangeCache) -> Option<Self> {
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
        // produce a second reference either. Nothing else touches `state`.
        unsafe { &mut *self.cache.state.get() }
    }
}

impl Drop for RenderGuard<'_> {
    fn drop(&mut self) {
        self.cache.rendering.store(false, Ordering::Release);
    }
}

/// Copies `message` to caller storage of `capacity` bytes, truncated and always NUL terminated.
///
/// # Safety
/// `output` is null or valid for writes of `capacity` bytes.
unsafe fn write_error(output: *mut c_char, capacity: u32, message: &str) {
    if output.is_null() || capacity == 0 {
        return;
    }
    let count = message.len().min(capacity as usize - 1);
    // SAFETY: `count + 1 <= capacity` bytes are written to storage the caller vouches for; it
    // cannot overlap a Rust string constant.
    unsafe {
        std::ptr::copy_nonoverlapping(message.as_ptr(), output.cast::<u8>(), count);
        output.add(count).write(0);
    }
}

/// Clears the caller's output: at most `maximum_request` samples per non-null channel, which
/// keeps rejection work bounded. Uses only immutable configuration, so it is safe while BUSY.
///
/// # Safety
/// `output` is null or points to `channels` pointers, each null or valid for writes of
/// `min(frames, maximum_request)` samples.
unsafe fn silence(cache: &STUnitRangeCache, frames: u32, output: *const *mut f32) {
    if output.is_null() {
        return;
    }
    let bounded = frames.min(cache.maximum_request) as usize;
    for channel in 0..cache.channels as usize {
        // SAFETY: `output` holds `channels` pointers; a non-null one is writable for `bounded`
        // samples by the caller's contract. All-zero bytes are 0.0f.
        unsafe {
            let samples = *output.add(channel);
            if !samples.is_null() {
                std::ptr::write_bytes(samples, 0, bounded);
            }
        }
    }
}

/// Records `error`, clears the output and returns `error`.
///
/// # Safety
/// As for [`silence`].
unsafe fn fail(
    cache: &STUnitRangeCache,
    stats: &mut STUnitRangeStats,
    error: i32,
    frames: u32,
    output: *const *mut f32,
) -> i32 {
    stats.last_error = error;
    // SAFETY: forwarded from the caller.
    unsafe { silence(cache, frames, output) };
    error
}

/// # Safety
/// `output` is null or points to `channels` pointers.
unsafe fn valid_output(cache: &STUnitRangeCache, output: *const *mut f32) -> bool {
    if output.is_null() {
        return false;
    }
    // SAFETY: `output` holds `channels` readable pointers by the caller's contract.
    (0..cache.channels as usize).all(|channel| unsafe { !(*output.add(channel)).is_null() })
}

/// Copies `frames` retained samples starting at graph frame `start` to the caller's output.
///
/// # Safety
/// `output` points to `channels` non-null pointers, each valid for writes of `frames` samples
/// and not overlapping the cache. `[start, start + frames)` lies within the retained range, and
/// `frames` is at most `maximum_request`.
unsafe fn copy_cached(
    cache: &STUnitRangeCache,
    state: &State,
    start: i64,
    frames: u32,
    output: *const *mut f32,
) {
    let history_frames = cache.history_frames;
    let offset = (start as u64).wrapping_sub(state.stats.retained_start_frame as u64);
    let oldest =
        (state.write_position + history_frames - state.stats.retained_frames) % history_frames;
    let position = (u64::from(oldest).wrapping_add(offset) % u64::from(history_frames)) as u32;
    let first = frames.min(history_frames - position) as usize;
    let (position, frames) = (position as usize, frames as usize);
    let channels = state
        .history
        .chunks_exact(history_frames as usize)
        .take(cache.channels as usize);
    for (channel, source) in channels.enumerate() {
        // SAFETY: `output[channel]` is writable for `frames` samples and disjoint from the ring.
        // `position < history_frames` and `first <= history_frames - position`, and the wrapped
        // remainder `frames - first` is at most `maximum_request <= history_frames`, so both
        // source ranges lie inside this channel's block.
        unsafe {
            let destination = *output.add(channel);
            std::ptr::copy_nonoverlapping(
                source[position..position + first].as_ptr(),
                destination,
                first,
            );
            if first < frames {
                std::ptr::copy_nonoverlapping(
                    source[..frames - first].as_ptr(),
                    destination.add(first),
                    frames - first,
                );
            }
        }
    }
}

/// Appends the first `frames` scratch samples of each channel to the ring; `end` is the graph
/// frame after them and `last` the context of the final span.
fn commit(
    cache: &STUnitRangeCache,
    state: &mut State,
    end: i64,
    frames: u32,
    last: &STClockContext,
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

/// `Box::new` that reports allocation failure instead of aborting.
fn try_box<T>(value: T) -> Option<Box<T>> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        return Some(Box::new(value));
    }
    // SAFETY: the layout has a non-zero size.
    let pointer = unsafe { alloc(layout) }.cast::<T>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: `pointer` is a fresh allocation from the global allocator with the layout of `T`,
    // which is exactly what a `Box<T>` owns; it is initialized before the box is formed.
    unsafe {
        pointer.write(value);
        Some(Box::from_raw(pointer))
    }
}

fn create(
    channels: u32,
    maximum_request_frames: u32,
    history_frames: u32,
    maximum_child_frames: u32,
    maximum_spans: u32,
) -> Result<Box<STUnitRangeCache>, &'static str> {
    if !(1..=2).contains(&channels) {
        return Err("Unit cache requires one or two channels.");
    }
    if !(1..=65536).contains(&maximum_request_frames) {
        return Err("Unit request capacity must be 1..65536 frames.");
    }
    if history_frames < maximum_request_frames || history_frames > 131072 {
        return Err("Unit history must cover one maximum request and not exceed 131072 frames.");
    }
    if maximum_child_frames < 1 || maximum_child_frames > maximum_request_frames {
        return Err("Child frame limit must fit the request capacity.");
    }
    if !(1..=256).contains(&maximum_spans) {
        return Err("Unit span capacity must be 1..256.");
    }
    if maximum_spans < maximum_request_frames.div_ceil(maximum_child_frames) {
        return Err(
            "Unit span capacity cannot cover the maximum request at the child frame limit.",
        );
    }
    const OUT_OF_MEMORY: &str = "Could not allocate bounded unit render storage.";
    let channel_count = channels as usize;
    let state = State {
        write_position: 0,
        content_offset: 0,
        sample_rate: 0.0,
        stats: STUnitRangeStats::default(),
        history: try_filled(channel_count * history_frames as usize).ok_or(OUT_OF_MEMORY)?,
        scratch: try_filled(channel_count * maximum_request_frames as usize)
            .ok_or(OUT_OF_MEMORY)?,
        plan: try_filled(maximum_spans as usize).ok_or(OUT_OF_MEMORY)?,
    };
    try_box(STUnitRangeCache {
        channels,
        maximum_request: maximum_request_frames,
        history_frames,
        maximum_child: maximum_child_frames,
        rendering: AtomicBool::new(false),
        state: UnsafeCell::new(state),
    })
    .ok_or(OUT_OF_MEMORY)
}

/// Control-thread allocation. channels=1/2, maximumRequestFrames=1..65536,
/// historyFrames=maximumRequestFrames..131072, maximumChildFrames=1..maximumRequestFrames,
/// maximumSpans=1..256. maximumSpans counts actual child calls, including splitting at
/// maximumChildFrames, and must cover ceil(maximumRequestFrames/maximumChildFrames).
/// Error text is optional and always NUL terminated.
///
/// # Safety
/// `error` is null or valid for writes of `error_capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn st_unit_range_create(
    channels: u32,
    maximum_request_frames: u32,
    history_frames: u32,
    maximum_child_frames: u32,
    maximum_spans: u32,
    error: *mut c_char,
    error_capacity: u32,
) -> *mut STUnitRangeCache {
    // SAFETY: the caller's contract for `error`, here and below.
    unsafe { write_error(error, error_capacity, "") };
    match create(
        channels,
        maximum_request_frames,
        history_frames,
        maximum_child_frames,
        maximum_spans,
    ) {
        Ok(cache) => Box::into_raw(cache),
        Err(message) => {
            unsafe { write_error(error, error_capacity, message) };
            std::ptr::null_mut()
        }
    }
}

/// Destroy only when callbacks are quiescent.
///
/// # Safety
/// `cache` is null or came from [`st_unit_range_create`], no call on it is in progress, and it is
/// not used again.
#[no_mangle]
pub unsafe extern "C" fn st_unit_range_destroy(cache: *mut STUnitRangeCache) {
    if !cache.is_null() {
        // SAFETY: the pointer came from `Box::into_raw` in `st_unit_range_create`; the caller
        // guarantees exclusive access and gives up ownership exactly once.
        drop(unsafe { Box::from_raw(cache) });
    }
}

/// Forgets all retained audio, the anchor and any latched error. Reset also clears statistics.
///
/// # Safety
/// `cache` is null or a live cache.
#[no_mangle]
pub unsafe extern "C" fn st_unit_range_reset(cache: *mut STUnitRangeCache) -> i32 {
    // SAFETY: a non-null `cache` is live by the caller's contract; only a shared reference is
    // formed (see the module documentation).
    let Some(cache) = (unsafe { cache.as_ref() }) else {
        return ST_UNIT_RANGE_INVALID;
    };
    let Some(mut guard) = RenderGuard::acquire(cache) else {
        return ST_UNIT_RANGE_BUSY;
    };
    let state = guard.state();
    state.stats = STUnitRangeStats::default();
    state.write_position = 0;
    state.content_offset = 0;
    state.sample_rate = 0.0;
    ST_UNIT_RANGE_OK
}

/// Bounded snapshot; returns BUSY without touching stats when rendering is active.
///
/// # Safety
/// `cache` is null or a live cache; `stats` is null or valid for a write.
#[no_mangle]
pub unsafe extern "C" fn st_unit_range_get_stats(
    cache: *mut STUnitRangeCache,
    stats: *mut STUnitRangeStats,
) -> i32 {
    // SAFETY: as in `st_unit_range_reset`.
    let Some(cache) = (unsafe { cache.as_ref() }) else {
        return ST_UNIT_RANGE_INVALID;
    };
    if stats.is_null() {
        return ST_UNIT_RANGE_INVALID;
    }
    let Some(mut guard) = RenderGuard::acquire(cache) else {
        return ST_UNIT_RANGE_BUSY;
    };
    // SAFETY: `stats` is non-null and writable by the caller's contract.
    unsafe { stats.write(guard.state().stats) };
    ST_UNIT_RANGE_OK
}

/// First success anchors any clock-valid graph frame. Further requests may repeat retained ranges
/// or overlap the retained prefix and append a contiguous suffix. Gaps, expired history, bad
/// spans, changed content offset/sample rate, and arithmetic overflow fail without changing good
/// cached PCM. Cached-only requests invoke no callbacks, including after a caller replaces its
/// clock. Revision/epoch changes at span boundaries are forwarded to the child, including within
/// one request; they do not invalidate old PCM. Changing sample rate or upstream content-frame
/// offset requires a quiescent reset. Spans must stay within their context's nextBoundaryFrame
/// and carry finite beat/bar/tempo values, positive tempo/numerator, and denominator 2/4/8/16.
///
/// Output is planar, one valid pointer per channel, with frames writable samples. Failures clear
/// output; invalid oversized calls clear only maximumRequestFrames samples per valid channel,
/// keeping rejection work bounded. A null cache cannot determine channels and therefore does not
/// touch output. Concurrent/reentrant render is rejected immediately. No render-time allocation,
/// locks or waiting.
///
/// # Safety
/// `cache` is null or a live cache. `output` is null or points to one pointer per channel, each
/// null or valid for writes of `frames` samples (of `maximumRequestFrames` samples when `frames`
/// is larger) and not overlapping the cache. The callbacks follow the rules above: they do not
/// unwind, `span` writes only the span it is given, and `process` writes at most `frames` samples
/// to each buffer it is given and nothing else of the cache.
#[no_mangle]
pub unsafe extern "C" fn st_unit_range_render(
    cache: *mut STUnitRangeCache,
    graph_frame: i64,
    frames: u32,
    output: *const *mut f32,
    prepare: STUnitRangePrepareFn,
    span: STUnitRangeSpanFn,
    process: STUnitRangeProcessFn,
    user: *mut c_void,
) -> i32 {
    // SAFETY: as in `st_unit_range_reset`. A reentrant call from a callback also only forms a
    // shared reference, which may coexist with the outer call's exclusive borrow of the state
    // cell's *contents* because it never looks inside the cell: it fails `acquire` below.
    let Some(cache) = (unsafe { cache.as_ref() }) else {
        return ST_UNIT_RANGE_INVALID;
    };
    let Some(mut guard) = RenderGuard::acquire(cache) else {
        // SAFETY: the caller's contract for `output`.
        unsafe { silence(cache, frames, output) };
        return ST_UNIT_RANGE_BUSY;
    };
    let state = guard.state();
    // Every rejection records the error and clears the caller's output.
    macro_rules! fail {
        ($error:expr) => {{
            let error = $error;
            // SAFETY: the caller's contract for `output`.
            unsafe { fail(cache, &mut state.stats, error, frames, output) }
        }};
    }
    state.stats.requests = state.stats.requests.wrapping_add(1);
    if frames == 0 || !unsafe { valid_output(cache, output) } {
        return fail!(ST_UNIT_RANGE_INVALID);
    }
    if frames > cache.maximum_request {
        return fail!(ST_UNIT_RANGE_OVERSIZE);
    }
    if graph_frame > i64::MAX - i64::from(frames) {
        return fail!(ST_UNIT_RANGE_OVERFLOW);
    }
    if state.stats.latched_error != 0 {
        return fail!(state.stats.latched_error);
    }
    let end = graph_frame + i64::from(frames);
    let mut suffix_start = graph_frame;
    if state.stats.anchored != 0 {
        if graph_frame < state.stats.retained_start_frame {
            return fail!(ST_UNIT_RANGE_EXPIRED);
        }
        if graph_frame > state.stats.rendered_end_frame {
            return fail!(ST_UNIT_RANGE_GAP);
        }
        if end <= state.stats.rendered_end_frame {
            // SAFETY: `output` was validated above, `frames <= maximum_request`, and the request
            // lies within [retained_start_frame, rendered_end_frame).
            unsafe { copy_cached(cache, state, graph_frame, frames, output) };
            state.stats.cache_hits = state.stats.cache_hits.wrapping_add(1);
            state.stats.successful_requests = state.stats.successful_requests.wrapping_add(1);
            state.stats.last_error = ST_UNIT_RANGE_OK;
            return ST_UNIT_RANGE_OK;
        }
        suffix_start = state.stats.rendered_end_frame;
    }
    let (Some(span), Some(process)) = (span, process) else {
        return fail!(ST_UNIT_RANGE_INVALID);
    };
    // graph_frame <= suffix_start < end, so this is 1..=frames.
    let suffix_frames = (end - suffix_start) as u32;

    // The native provider must first retain the new audio/clock/event range. This
    // is deliberately separate from per-child event dispatch in process below.
    if let Some(prepare) = prepare {
        // SAFETY: the caller's callback, called as the header documents.
        let result = unsafe { prepare(user, suffix_start, suffix_frames) };
        if result != 0 {
            return fail!(result);
        }
        state.stats.prepared_frames = state
            .stats
            .prepared_frames
            .wrapping_add(u64::from(suffix_frames));
    }

    let mut span_count: usize = 0;
    let mut planned: u32 = 0;
    let mut sample_rate = state.sample_rate;
    let mut content_offset = state.content_offset;
    let mut have_format = state.stats.anchored != 0;
    while planned < suffix_frames {
        // The plan holds exactly maximumSpans entries.
        let Some(part) = state.plan.get_mut(span_count) else {
            return fail!(ST_UNIT_RANGE_SPAN_LIMIT);
        };
        let cursor = suffix_start + i64::from(planned);
        let requested = (suffix_frames - planned).min(cache.maximum_child);
        *part = STClockSpan::default();
        // SAFETY: the caller's callback, given a valid span to fill in. `part` is not used again
        // until the callback has returned.
        let result = unsafe { span(user, cursor, requested, part) };
        if result != ST_CLOCK_OK {
            let error = if result == ST_CLOCK_OUTSIDE_DOMAIN {
                ST_UNIT_RANGE_DOMAIN_END
            } else {
                ST_UNIT_RANGE_CLOCK_ERROR
            };
            return fail!(error);
        }
        if part.frames == 0 {
            return fail!(ST_UNIT_RANGE_DOMAIN_END);
        }
        if part.frames > requested {
            return fail!(ST_UNIT_RANGE_CONTEXT_MISMATCH);
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
            return fail!(ST_UNIT_RANGE_CONTEXT_MISMATCH);
        }
        let offset = (context.graph_frame as u64).wrapping_sub(context.content_graph_frame as u64);
        if offset > 1 << 30 {
            return fail!(ST_UNIT_RANGE_CONTEXT_MISMATCH);
        }
        if !have_format {
            sample_rate = context.sample_rate;
            content_offset = offset as i64;
            have_format = true;
        } else if context.sample_rate != sample_rate || offset as i64 != content_offset {
            return fail!(ST_UNIT_RANGE_CONTEXT_MISMATCH);
        }
        planned += part.frames;
        span_count += 1;
    }

    // Cache publication is transactional. A child may already have advanced when
    // it reports failure, so its error is latched; scratch PCM is never committed.
    let maximum_request = cache.maximum_request as usize;
    for channel in state
        .scratch
        .chunks_exact_mut(maximum_request)
        .take(cache.channels as usize)
    {
        // suffix_frames <= frames <= maximum_request.
        channel[..suffix_frames as usize].fill(0.0);
    }
    let scratch = state.scratch.as_mut_ptr();
    let mut processed: u32 = 0;
    for part in state.plan.iter().take(span_count) {
        // SAFETY: the plan's spans add up to suffix_frames <= maximum_request, so `processed` and
        // `processed + part.frames` stay within one channel's block of `maximum_request` samples;
        // with two channels the scratch storage holds two such blocks.
        let buffers: [*mut f32; 2] = unsafe {
            [
                scratch.add(processed as usize),
                if cache.channels == 2 {
                    scratch.add(maximum_request + processed as usize)
                } else {
                    std::ptr::null_mut()
                },
            ]
        };
        state.stats.process_calls = state.stats.process_calls.wrapping_add(1);
        // SAFETY: the caller's callback. It receives a read-only view of this span's context and
        // buffers with room for `part.frames` samples per channel. The scratch storage is not
        // accessed through `state` again until the callback has returned.
        let result = unsafe {
            process(
                user,
                &part.context,
                part.context.graph_frame,
                part.frames,
                buffers.as_ptr(),
            )
        };
        if result != 0 {
            state.stats.latched_error = result;
            return fail!(result);
        }
        state.stats.processed_frames = state
            .stats
            .processed_frames
            .wrapping_add(u64::from(part.frames));
        processed += part.frames;
    }
    // suffix_frames >= 1, so at least one span was planned.
    let last = state
        .plan
        .get(span_count.saturating_sub(1))
        .map_or_else(STClockContext::default, |part| part.context);
    commit(
        cache,
        state,
        end,
        suffix_frames,
        &last,
        sample_rate,
        content_offset,
    );
    // SAFETY: `output` was validated above, `frames <= maximum_request`, and after the commit the
    // request lies within the retained range.
    unsafe { copy_cached(cache, state, graph_frame, frames, output) };
    state.stats.successful_requests = state.stats.successful_requests.wrapping_add(1);
    state.stats.last_error = ST_UNIT_RANGE_OK;
    ST_UNIT_RANGE_OK
}

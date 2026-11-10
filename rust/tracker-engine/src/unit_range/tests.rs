//! Unit tests for the unit range cache.
//!
//! `tests/unit_range_tests.cpp` remains the acceptance test and is run against this library
//! unchanged. These tests cover what it leaves out: the exact creation messages, the header's
//! layout, mono caches, rejection paths with missing output pointers, and reentrancy from the
//! `prepare` and `span` callbacks.

use std::ffi::{c_char, c_void, CStr};
use std::mem::{align_of, offset_of, size_of, size_of_val};
use std::ptr::{null, null_mut};

use super::*;

/// The output of a C++ program printing `sizeof`, `alignof` and `offsetof` for the struct of
/// `TrackerUnitRange.h` (identical for x86-64 and arm64).
const CXX_LAYOUT: &str = "\
STUnitRangeStats size 96 align 8
STUnitRangeStats.requests offset 0 size 8
STUnitRangeStats.successfulRequests offset 8 size 8
STUnitRangeStats.cacheHits offset 16 size 8
STUnitRangeStats.preparedFrames offset 24 size 8
STUnitRangeStats.processedFrames offset 32 size 8
STUnitRangeStats.processCalls offset 40 size 8
STUnitRangeStats.retainedStartFrame offset 48 size 8
STUnitRangeStats.renderedEndFrame offset 56 size 8
STUnitRangeStats.revision offset 64 size 8
STUnitRangeStats.epoch offset 72 size 8
STUnitRangeStats.retainedFrames offset 80 size 4
STUnitRangeStats.anchored offset 84 size 4
STUnitRangeStats.lastError offset 88 size 4
STUnitRangeStats.latchedError offset 92 size 4
unit range constants 0 -74000 -74001 -74002 -74003 -74004 -74005 -74006 -74007 -74008 -74009
";

#[test]
fn layout_matches_the_c_header() {
    let value = STUnitRangeStats::default();
    let mut report = format!(
        "STUnitRangeStats size {} align {}\n",
        size_of::<STUnitRangeStats>(),
        align_of::<STUnitRangeStats>()
    );
    macro_rules! field {
        ($c_name:literal, $field:ident) => {
            report += &format!(
                "STUnitRangeStats.{} offset {} size {}\n",
                $c_name,
                offset_of!(STUnitRangeStats, $field),
                size_of_val(&value.$field)
            );
        };
    }
    field!("requests", requests);
    field!("successfulRequests", successful_requests);
    field!("cacheHits", cache_hits);
    field!("preparedFrames", prepared_frames);
    field!("processedFrames", processed_frames);
    field!("processCalls", process_calls);
    field!("retainedStartFrame", retained_start_frame);
    field!("renderedEndFrame", rendered_end_frame);
    field!("revision", revision);
    field!("epoch", epoch);
    field!("retainedFrames", retained_frames);
    field!("anchored", anchored);
    field!("lastError", last_error);
    field!("latchedError", latched_error);
    report += &format!(
        "unit range constants {ST_UNIT_RANGE_OK} {ST_UNIT_RANGE_INVALID} {ST_UNIT_RANGE_OVERSIZE} \
         {ST_UNIT_RANGE_OVERFLOW} {ST_UNIT_RANGE_GAP} {ST_UNIT_RANGE_EXPIRED} {ST_UNIT_RANGE_BUSY} \
         {ST_UNIT_RANGE_CLOCK_ERROR} {ST_UNIT_RANGE_DOMAIN_END} {ST_UNIT_RANGE_CONTEXT_MISMATCH} \
         {ST_UNIT_RANGE_SPAN_LIMIT}\n"
    );
    print!("{report}");
    assert_eq!(report, CXX_LAYOUT);
    // The span type the callbacks exchange is the clock's.
    assert_eq!(
        (size_of::<STClockSpan>(), size_of::<STClockContext>()),
        (168, 160)
    );
}

#[test]
fn creation_reports_each_error_text() {
    let cases: [([u32; 5], &str); 12] = [
        ([0, 8, 8, 8, 1], "Unit cache requires one or two channels."),
        ([3, 8, 8, 8, 1], "Unit cache requires one or two channels."),
        (
            [1, 0, 8, 8, 1],
            "Unit request capacity must be 1..65536 frames.",
        ),
        (
            [1, 65537, 65537, 8, 1],
            "Unit request capacity must be 1..65536 frames.",
        ),
        (
            [1, 8, 7, 8, 1],
            "Unit history must cover one maximum request and not exceed 131072 frames.",
        ),
        (
            [1, 8, 131073, 8, 1],
            "Unit history must cover one maximum request and not exceed 131072 frames.",
        ),
        (
            [1, 8, 8, 0, 1],
            "Child frame limit must fit the request capacity.",
        ),
        (
            [1, 8, 8, 9, 1],
            "Child frame limit must fit the request capacity.",
        ),
        ([1, 8, 8, 8, 0], "Unit span capacity must be 1..256."),
        ([1, 8, 8, 8, 257], "Unit span capacity must be 1..256."),
        (
            [1, 16, 16, 4, 3],
            "Unit span capacity cannot cover the maximum request at the child frame limit.",
        ),
        (
            [u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX],
            "Unit cache requires one or two channels.",
        ),
    ];
    for ([channels, request, history, child, spans], expected) in cases {
        let mut error = [b'x'; 128];
        // SAFETY: the error storage is live and as large as stated.
        let cache = unsafe {
            st_unit_range_create(
                channels,
                request,
                history,
                child,
                spans,
                error.as_mut_ptr().cast(),
                error.len() as u32,
            )
        };
        assert!(cache.is_null());
        assert_eq!(
            CStr::from_bytes_until_nul(&error)
                .unwrap()
                .to_str()
                .unwrap(),
            expected
        );
    }
    // SAFETY: as above; null error storage is allowed whatever the capacity says.
    unsafe {
        let mut small = [b'x'; 8];
        assert!(st_unit_range_create(0, 8, 8, 8, 1, small.as_mut_ptr().cast(), 5).is_null());
        assert_eq!(&small, b"Unit\0xxx");
        assert!(st_unit_range_create(0, 8, 8, 8, 1, small.as_mut_ptr().cast(), 0).is_null());
        assert_eq!(&small, b"Unit\0xxx");
        assert!(st_unit_range_create(0, 8, 8, 8, 1, null_mut(), 64).is_null());
        // Success clears the text. The largest configuration is accepted.
        let mut error = [b'x' as c_char; 16];
        let cache = st_unit_range_create(2, 65536, 131072, 65536, 256, error.as_mut_ptr(), 16);
        assert!(!cache.is_null());
        assert_eq!(error[0], 0);
        st_unit_range_destroy(cache);
        st_unit_range_destroy(null_mut());
    }
}

/// A child that writes a running sample count, and records what the cache asked of it.
struct Child {
    cache: *mut STUnitRangeCache,
    advanced: u32,
    prepared: Vec<(i64, u32)>,
    planned: Vec<(i64, u32)>,
    processed: Vec<(i64, u32)>,
    /// Spans end at multiples of this many frames (0: only where the cache asks).
    partition: i64,
    second_buffer_was_null: bool,
    reenter_from_prepare: bool,
    reenter_from_span: bool,
    reentered: u32,
}

impl Child {
    fn new(channels: u32, request: u32, history: u32, child: u32, spans: u32) -> Self {
        // SAFETY: no error storage is passed.
        let cache = unsafe {
            st_unit_range_create(channels, request, history, child, spans, null_mut(), 0)
        };
        assert!(!cache.is_null());
        Self {
            cache,
            advanced: 0,
            prepared: Vec::new(),
            planned: Vec::new(),
            processed: Vec::new(),
            partition: 0,
            second_buffer_was_null: false,
            reenter_from_prepare: false,
            reenter_from_span: false,
            reentered: 0,
        }
    }

    /// Every entry point must refuse to run while a render is in progress, without waiting.
    unsafe fn expect_busy(&mut self) {
        let mut samples = [7.0f32; 4];
        let output = [samples.as_mut_ptr(), null_mut()];
        // SAFETY: called from a callback of a live cache; the output is live local storage (the
        // second, null channel must be skipped), and no callbacks are needed to be told BUSY.
        unsafe {
            assert_eq!(
                st_unit_range_render(
                    self.cache,
                    0,
                    4,
                    output.as_ptr(),
                    None,
                    None,
                    None,
                    null_mut()
                ),
                ST_UNIT_RANGE_BUSY
            );
            assert_eq!(samples, [0.0; 4]);
            // A null output is left alone.
            assert_eq!(
                st_unit_range_render(self.cache, 0, 4, null(), None, None, None, null_mut()),
                ST_UNIT_RANGE_BUSY
            );
            let mut stats = STUnitRangeStats {
                requests: 123,
                ..STUnitRangeStats::default()
            };
            assert_eq!(
                st_unit_range_get_stats(self.cache, &mut stats),
                ST_UNIT_RANGE_BUSY
            );
            assert_eq!(stats.requests, 123);
            assert_eq!(st_unit_range_reset(self.cache), ST_UNIT_RANGE_BUSY);
        }
        self.reentered += 1;
    }

    unsafe extern "C" fn prepare(user: *mut c_void, start: i64, frames: u32) -> i32 {
        // SAFETY: `user` is the `Child` passed to render, which does not touch it meanwhile.
        let child = unsafe { &mut *user.cast::<Child>() };
        child.prepared.push((start, frames));
        if child.reenter_from_prepare {
            // SAFETY: inside a callback of the live cache.
            unsafe { child.expect_busy() };
        }
        0
    }

    unsafe extern "C" fn span(
        user: *mut c_void,
        start: i64,
        frames: u32,
        span: *mut STClockSpan,
    ) -> i32 {
        // SAFETY: as in `prepare`; `span` is the cache's valid plan entry.
        let (child, span) = unsafe { (&mut *user.cast::<Child>(), &mut *span) };
        if child.reenter_from_span {
            // SAFETY: inside a callback of the live cache.
            unsafe { child.expect_busy() };
        }
        let mut count = frames;
        if child.partition > 0 {
            count = count.min((child.partition - start.rem_euclid(child.partition)) as u32);
        }
        span.frames = count;
        span.context = STClockContext {
            graph_frame: start,
            content_graph_frame: start,
            song_frame: start,
            sample_rate: 48000.0,
            bpm: 120.0,
            numerator: 4,
            denominator: 4,
            revision: 5,
            epoch: 9,
            next_boundary_frame: start + i64::from(count),
            ..STClockContext::default()
        };
        child.planned.push((start, count));
        clock_abi::ST_CLOCK_OK
    }

    unsafe extern "C" fn process(
        user: *mut c_void,
        context: *const STClockContext,
        start: i64,
        frames: u32,
        output: *const *mut f32,
    ) -> i32 {
        // SAFETY: as in `prepare`; `context` and the two buffer pointers are valid for reads, and
        // each non-null buffer for `frames` writes.
        unsafe {
            let child = &mut *user.cast::<Child>();
            assert_eq!((*context).graph_frame, start);
            child.processed.push((start, frames));
            let (left, right) = (*output, *output.add(1));
            child.second_buffer_was_null = right.is_null();
            for index in 0..frames as usize {
                child.advanced += 1;
                left.add(index).write(child.advanced as f32);
                if !right.is_null() {
                    right.add(index).write(-(child.advanced as f32));
                }
            }
        }
        0
    }

    /// Renders into fresh buffers pre-filled with 99 and returns (status, left, right).
    fn render(&mut self, start: i64, frames: u32, capacity: usize) -> (i32, Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![99.0f32; capacity], vec![99.0f32; capacity]);
        let output = [left.as_mut_ptr(), right.as_mut_ptr()];
        let user: *mut Child = self;
        // SAFETY: the cache is live, the buffers hold `capacity` samples (the tests keep
        // `min(frames, maximum request)` within it), and the callbacks only use `user` as `Child`.
        let status = unsafe {
            st_unit_range_render(
                self.cache,
                start,
                frames,
                output.as_ptr(),
                Some(Self::prepare),
                Some(Self::span),
                Some(Self::process),
                user.cast(),
            )
        };
        (status, left, right)
    }

    fn stats(&self) -> STUnitRangeStats {
        let mut stats = STUnitRangeStats::default();
        // SAFETY: the cache is live and `stats` is a live local.
        assert_eq!(
            unsafe { st_unit_range_get_stats(self.cache, &mut stats) },
            ST_UNIT_RANGE_OK
        );
        stats
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: the cache came from `st_unit_range_create` and no render is in progress.
        unsafe { st_unit_range_destroy(self.cache) };
    }
}

fn ramp(first: u32, count: u32) -> Vec<f32> {
    (first..first + count).map(|value| value as f32).collect()
}

#[test]
fn a_mono_cache_renders_only_the_uncached_suffix() {
    let mut child = Child::new(1, 8, 16, 4, 4);
    let (status, left, right) = child.render(-3, 8, 10);
    assert_eq!(status, ST_UNIT_RANGE_OK);
    assert_eq!(left[..8], ramp(1, 8)[..]);
    // Nothing beyond the request, and nothing of a channel the cache does not have, is written.
    assert_eq!((left[8], left[9]), (99.0, 99.0));
    assert_eq!(right, vec![99.0; 10]);
    assert!(child.second_buffer_was_null);
    assert_eq!(child.prepared, [(-3, 8)]);
    assert_eq!(child.processed, [(-3, 4), (1, 4)]);
    // Overlap: the retained prefix is copied, only the new frames reach the child.
    let (status, left, _) = child.render(1, 8, 8);
    assert_eq!(status, ST_UNIT_RANGE_OK);
    assert_eq!(left, ramp(5, 8));
    assert_eq!(child.prepared, [(-3, 8), (5, 4)]);
    assert_eq!(child.processed, [(-3, 4), (1, 4), (5, 4)]);
    // Entirely retained: no callback at all.
    let (status, left, _) = child.render(-3, 8, 8);
    assert_eq!(status, ST_UNIT_RANGE_OK);
    assert_eq!(left, ramp(1, 8));
    assert_eq!(child.planned.len(), 3);
    let stats = child.stats();
    assert_eq!(
        stats,
        STUnitRangeStats {
            requests: 3,
            successful_requests: 3,
            cache_hits: 1,
            prepared_frames: 12,
            processed_frames: 12,
            process_calls: 3,
            retained_start_frame: -3,
            rendered_end_frame: 9,
            revision: 5,
            epoch: 9,
            retained_frames: 12,
            anchored: 1,
            last_error: 0,
            latched_error: 0,
        }
    );
}

#[test]
fn stereo_history_wraps_per_channel() {
    let mut child = Child::new(2, 8, 11, 8, 8);
    child.partition = 5;
    let mut frontier = 1000i64;
    for step in 0..40u32 {
        let extra = 1 + step % 7;
        let overlap = (step % 4).min((frontier - 1000) as u32).min(8 - extra);
        let (status, left, right) = child.render(frontier - i64::from(overlap), overlap + extra, 8);
        assert_eq!(status, ST_UNIT_RANGE_OK, "step {step}");
        let first = (frontier - 1000) as u32 - overlap + 1;
        let expected = ramp(first, overlap + extra);
        assert_eq!(left[..expected.len()], expected[..], "step {step}");
        assert!(
            right[..expected.len()]
                .iter()
                .zip(&expected)
                .all(|(actual, wanted)| *actual == -wanted),
            "step {step}"
        );
        frontier += i64::from(extra);
    }
    assert!(!child.second_buffer_was_null);
    assert!(child
        .processed
        .iter()
        .all(|&(start, frames)| start.rem_euclid(5) + i64::from(frames) <= 5));
    assert_eq!(child.stats().retained_frames, 11);
    assert_eq!(child.stats().retained_start_frame, frontier - 11);
}

#[test]
fn rejections_clear_the_output_and_keep_the_cache() {
    let mut child = Child::new(2, 8, 16, 8, 2);
    assert_eq!(child.render(0, 8, 8).0, ST_UNIT_RANGE_OK);
    let calls = child.processed.len();
    // No frames: invalid, and nothing is written.
    let (status, left, _) = child.render(0, 0, 4);
    assert_eq!((status, left), (ST_UNIT_RANGE_INVALID, vec![99.0; 4]));
    assert_eq!(child.stats().last_error, ST_UNIT_RANGE_INVALID);
    // Oversized: only the request capacity is cleared, whatever the frame count claims. The size
    // is checked before the arithmetic range.
    for (start, frames) in [
        (0, 9),
        (0, u32::MAX),
        (i64::MAX, u32::MAX),
        (i64::MIN, 65537),
    ] {
        let (status, left, right) = child.render(start, frames, 10);
        assert_eq!(status, ST_UNIT_RANGE_OVERSIZE);
        assert_eq!(left, [[0.0; 8].as_slice(), &[99.0; 2]].concat());
        assert_eq!(right, left);
    }
    for (start, frames, expected) in [
        (i64::MAX - 7, 8, ST_UNIT_RANGE_OVERFLOW),
        (i64::MAX, 1, ST_UNIT_RANGE_OVERFLOW),
        (-1, 4, ST_UNIT_RANGE_EXPIRED),
        (i64::MIN, 8, ST_UNIT_RANGE_EXPIRED),
        (9, 4, ST_UNIT_RANGE_GAP),
        (i64::MAX - 8, 8, ST_UNIT_RANGE_GAP),
    ] {
        let (status, left, right) = child.render(start, frames, 8);
        assert_eq!(status, expected, "{start}+{frames}");
        assert!(left[..frames as usize]
            .iter()
            .chain(&right[..frames as usize])
            .all(|sample| *sample == 0.0));
        assert_eq!(child.stats().last_error, expected);
    }
    // SAFETY: the cache is live; each output array holds two pointers that are null or address
    // eight live samples; no callbacks are passed.
    unsafe {
        let mut samples = [99.0f32; 8];
        let partial = [null_mut(), samples.as_mut_ptr()];
        assert_eq!(
            st_unit_range_render(
                child.cache,
                0,
                8,
                partial.as_ptr(),
                None,
                None,
                None,
                null_mut()
            ),
            ST_UNIT_RANGE_INVALID
        );
        // The channel that is present is still cleared.
        assert_eq!(samples, [0.0; 8]);
        assert_eq!(
            st_unit_range_render(child.cache, 0, 8, null(), None, None, None, null_mut()),
            ST_UNIT_RANGE_INVALID
        );
        assert_eq!(
            st_unit_range_render(
                null_mut(),
                0,
                8,
                partial.as_ptr(),
                None,
                None,
                None,
                null_mut()
            ),
            ST_UNIT_RANGE_INVALID
        );
        assert_eq!(
            st_unit_range_get_stats(child.cache, null_mut()),
            ST_UNIT_RANGE_INVALID
        );
        assert_eq!(
            st_unit_range_get_stats(null_mut(), null_mut()),
            ST_UNIT_RANGE_INVALID
        );
        assert_eq!(st_unit_range_reset(null_mut()), ST_UNIT_RANGE_INVALID);
    }
    // None of this reached the child or disturbed what was retained.
    assert_eq!(child.processed.len(), calls);
    let (status, left, _) = child.render(0, 8, 8);
    assert_eq!((status, left), (ST_UNIT_RANGE_OK, ramp(1, 8)));
    let stats = child.stats();
    assert_eq!(
        (stats.requests, stats.successful_requests, stats.cache_hits),
        (15, 2, 1)
    );
    assert_eq!((stats.last_error, stats.latched_error), (0, 0));
}

#[test]
fn reentry_from_prepare_and_span_is_rejected_as_busy() {
    let mut child = Child::new(2, 8, 16, 4, 4);
    child.reenter_from_prepare = true;
    child.reenter_from_span = true;
    let (status, left, _) = child.render(0, 8, 8);
    assert_eq!((status, left), (ST_UNIT_RANGE_OK, ramp(1, 8)));
    // Once from prepare and once from each of the two span queries.
    assert_eq!(child.reentered, 3);
    // The flag is released again on every path out of a render.
    assert_eq!(child.stats().requests, 1);
    // SAFETY: the cache is live.
    assert_eq!(
        unsafe { st_unit_range_reset(child.cache) },
        ST_UNIT_RANGE_OK
    );
    assert_eq!(child.stats(), STUnitRangeStats::default());
}

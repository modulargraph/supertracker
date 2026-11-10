//! Unit tests for the unit range cache.
//!
//! `tests/unit_range.rs` is the acceptance test. These tests cover what it leaves out: the exact
//! creation messages, the error codes, mono caches, rejection paths with output buffers that do
//! not fit, and reentrancy from the `prepare` and `span` steps of a source.

use super::*;
use std::sync::Arc;

#[test]
fn error_codes_are_stable_and_round_trip() {
    let codes: Vec<i32> = [
        UnitRangeError::Invalid,
        UnitRangeError::Oversize,
        UnitRangeError::Overflow,
        UnitRangeError::Gap,
        UnitRangeError::Expired,
        UnitRangeError::Busy,
        UnitRangeError::ClockError,
        UnitRangeError::DomainEnd,
        UnitRangeError::ContextMismatch,
        UnitRangeError::SpanLimit,
    ]
    .iter()
    .map(|error| {
        assert_eq!(UnitRangeError::from_code(error.code()), Some(*error));
        assert!(!error.to_string().is_empty());
        error.code()
    })
    .collect();
    assert_eq!(codes, [-74000, -74001, -74002, -74003, -74004, -74005, -74006, -74007, -74008, -74009]);
    assert_eq!(UnitRangeError::from_code(0), None);
    let child = UnitRangeError::from_code(-12345).expect("a nonzero code");
    assert_eq!((child, child.code()), (UnitRangeError::Source(NonZeroI32::new(-12345).unwrap()), -12345));
    assert_eq!(child.to_string(), "The unit failed with status -12345.");
}

#[test]
fn creation_reports_each_error_text() {
    let cases: [([u32; 5], &str); 12] = [
        ([0, 8, 8, 8, 1], "Unit cache requires one or two channels."),
        ([3, 8, 8, 8, 1], "Unit cache requires one or two channels."),
        ([1, 0, 8, 8, 1], "Unit request capacity must be 1..65536 frames."),
        ([1, 65537, 65537, 8, 1], "Unit request capacity must be 1..65536 frames."),
        ([1, 8, 7, 8, 1], "Unit history must cover one maximum request and not exceed 131072 frames."),
        ([1, 8, 131073, 8, 1], "Unit history must cover one maximum request and not exceed 131072 frames."),
        ([1, 8, 8, 0, 1], "Child frame limit must fit the request capacity."),
        ([1, 8, 8, 9, 1], "Child frame limit must fit the request capacity."),
        ([1, 8, 8, 8, 0], "Unit span capacity must be 1..256."),
        ([1, 8, 8, 8, 257], "Unit span capacity must be 1..256."),
        ([1, 16, 16, 4, 3], "Unit span capacity cannot cover the maximum request at the child frame limit."),
        ([u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX], "Unit cache requires one or two channels."),
    ];
    for ([channels, request, history, child, spans], expected) in cases {
        let error = UnitRangeCache::new(channels, request, history, child, spans).err().expect("an invalid configuration");
        assert_eq!((error.message(), error.to_string().as_str()), (expected, expected));
    }
    // The largest configuration is accepted.
    let cache = UnitRangeCache::new(2, 65536, 131072, 65536, 256).expect("the largest cache");
    assert_eq!((cache.channels(), cache.maximum_request_frames()), (2, 65536));
}

/// A child that writes a running sample count, and records what the cache asked of it.
struct Child {
    cache: Arc<UnitRangeCache>,
    advanced: u32,
    prepared: Vec<(i64, u32)>,
    planned: Vec<(i64, u32)>,
    processed: Vec<(i64, u32)>,
    /// Spans end at multiples of this many frames (0: only where the cache asks).
    partition: i64,
    /// Channels the cache handed to `process` last.
    process_channels: usize,
    reenter_from_prepare: bool,
    reenter_from_span: bool,
    reentered: u32,
}

impl Child {
    fn new(channels: u32, request: u32, history: u32, child: u32, spans: u32) -> Self {
        let cache = UnitRangeCache::new(channels, request, history, child, spans).expect("a valid cache");
        Self {
            cache: Arc::new(cache),
            advanced: 0,
            prepared: Vec::new(),
            planned: Vec::new(),
            processed: Vec::new(),
            partition: 0,
            process_channels: 0,
            reenter_from_prepare: false,
            reenter_from_span: false,
            reentered: 0,
        }
    }

    /// Every entry point must refuse to run while a render is in progress, without waiting.
    fn expect_busy(&mut self) {
        let mut samples = [7.0f32; 4];
        // One buffer for a stereo cache: the channel that is there is still cleared.
        assert_eq!(self.cache.render(0, 4, &mut [&mut samples], None), Err(UnitRangeError::Busy));
        assert_eq!(samples, [0.0; 4]);
        // No output at all is left alone.
        assert_eq!(self.cache.render(0, 4, &mut [], None), Err(UnitRangeError::Busy));
        assert_eq!(self.cache.stats(), Err(UnitRangeError::Busy));
        assert_eq!(self.cache.reset(), Err(UnitRangeError::Busy));
        self.reentered += 1;
    }

    /// Renders into fresh buffers pre-filled with 99 and returns (code, left, right).
    fn render(&mut self, start: i64, frames: u32, capacity: usize) -> (i32, Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![99.0f32; capacity], vec![99.0f32; capacity]);
        let cache = self.cache.clone();
        let result = cache.render(start, frames, &mut [&mut left, &mut right], Some(self));
        (result.map_or_else(UnitRangeError::code, |()| 0), left, right)
    }

    fn stats(&self) -> UnitRangeStats {
        self.cache.stats().expect("no render is in progress")
    }
}

impl UnitRangeSource for Child {
    fn prepare(&mut self, start: i64, frames: u32) -> Result<(), NonZeroI32> {
        self.prepared.push((start, frames));
        if self.reenter_from_prepare {
            self.expect_busy();
        }
        Ok(())
    }

    fn span(&mut self, start: i64, frames: u32) -> Result<ClockSpan, ClockQueryError> {
        if self.reenter_from_span {
            self.expect_busy();
        }
        let mut count = frames;
        if self.partition > 0 {
            count = count.min((self.partition - start.rem_euclid(self.partition)) as u32);
        }
        self.planned.push((start, count));
        Ok(ClockSpan {
            frames: count,
            context: ClockContext {
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
                ..ClockContext::default()
            },
        })
    }

    fn process(&mut self, context: &ClockContext, start: i64, frames: u32, output: &mut [&mut [f32]]) -> Result<(), NonZeroI32> {
        assert_eq!(context.graph_frame, start);
        assert!(output.iter().all(|channel| channel.len() == frames as usize && channel.iter().all(|sample| *sample == 0.0)));
        self.processed.push((start, frames));
        self.process_channels = output.len();
        for index in 0..frames as usize {
            self.advanced += 1;
            output[0][index] = self.advanced as f32;
            if let Some(right) = output.get_mut(1) {
                right[index] = -(self.advanced as f32);
            }
        }
        Ok(())
    }
}

fn ramp(first: u32, count: u32) -> Vec<f32> {
    (first..first + count).map(|value| value as f32).collect()
}

const OK: i32 = 0;

#[test]
fn a_mono_cache_renders_only_the_uncached_suffix() {
    let mut child = Child::new(1, 8, 16, 4, 4);
    let (status, left, right) = child.render(-3, 8, 10);
    assert_eq!(status, OK);
    assert_eq!(left[..8], ramp(1, 8)[..]);
    // Nothing beyond the request, and nothing of a channel the cache does not have, is written.
    assert_eq!((left[8], left[9]), (99.0, 99.0));
    assert_eq!(right, vec![99.0; 10]);
    assert_eq!(child.process_channels, 1);
    assert_eq!(child.prepared, [(-3, 8)]);
    assert_eq!(child.processed, [(-3, 4), (1, 4)]);
    // Overlap: the retained prefix is copied, only the new frames reach the child.
    let (status, left, _) = child.render(1, 8, 8);
    assert_eq!(status, OK);
    assert_eq!(left, ramp(5, 8));
    assert_eq!(child.prepared, [(-3, 8), (5, 4)]);
    assert_eq!(child.processed, [(-3, 4), (1, 4), (5, 4)]);
    // Entirely retained: the source is not used at all.
    let (status, left, _) = child.render(-3, 8, 8);
    assert_eq!(status, OK);
    assert_eq!(left, ramp(1, 8));
    assert_eq!(child.planned.len(), 3);
    let stats = child.stats();
    assert_eq!(
        stats,
        UnitRangeStats {
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
        assert_eq!(status, OK, "step {step}");
        let first = (frontier - 1000) as u32 - overlap + 1;
        let expected = ramp(first, overlap + extra);
        assert_eq!(left[..expected.len()], expected[..], "step {step}");
        assert!(right[..expected.len()].iter().zip(&expected).all(|(actual, wanted)| *actual == -wanted), "step {step}");
        frontier += i64::from(extra);
    }
    assert_eq!(child.process_channels, 2);
    assert!(child.processed.iter().all(|&(start, frames)| start.rem_euclid(5) + i64::from(frames) <= 5));
    assert_eq!(child.stats().retained_frames, 11);
    assert_eq!(child.stats().retained_start_frame, frontier - 11);
}

#[test]
fn rejections_clear_the_output_and_keep_the_cache() {
    let mut child = Child::new(2, 8, 16, 8, 2);
    assert_eq!(child.render(0, 8, 8).0, OK);
    let calls = child.processed.len();
    // No frames: invalid, and nothing is written.
    let (status, left, _) = child.render(0, 0, 4);
    assert_eq!((status, left), (UnitRangeError::Invalid.code(), vec![99.0; 4]));
    assert_eq!(child.stats().last_error, UnitRangeError::Invalid.code());
    // Oversized: only the request capacity is cleared, whatever the frame count claims. The size
    // is checked before the arithmetic range.
    for (start, frames) in [(0, 9), (0, u32::MAX), (i64::MAX, u32::MAX), (i64::MIN, 65537)] {
        let (status, left, right) = child.render(start, frames, 10);
        assert_eq!(status, UnitRangeError::Oversize.code());
        assert_eq!(left, [[0.0; 8].as_slice(), &[99.0; 2]].concat());
        assert_eq!(right, left);
    }
    for (start, frames, expected) in [
        (i64::MAX - 7, 8, UnitRangeError::Overflow),
        (i64::MAX, 1, UnitRangeError::Overflow),
        (-1, 4, UnitRangeError::Expired),
        (i64::MIN, 8, UnitRangeError::Expired),
        (9, 4, UnitRangeError::Gap),
        (i64::MAX - 8, 8, UnitRangeError::Gap),
    ] {
        let (status, left, right) = child.render(start, frames, 8);
        assert_eq!(status, expected.code(), "{start}+{frames}");
        assert!(left[..frames as usize].iter().chain(&right[..frames as usize]).all(|sample| *sample == 0.0));
        assert_eq!(child.stats().last_error, expected.code());
    }
    let cache = child.cache.clone();
    // One buffer for two channels: invalid; the channel that is present is still cleared.
    let mut samples = [99.0f32; 8];
    assert_eq!(cache.render(0, 8, &mut [&mut samples], None), Err(UnitRangeError::Invalid));
    assert_eq!(samples, [0.0; 8]);
    // No buffers.
    assert_eq!(cache.render(0, 8, &mut [], None), Err(UnitRangeError::Invalid));
    // A buffer shorter than the request: invalid, and what there is of it is cleared.
    let (mut long, mut short) = ([99.0f32; 8], [99.0f32; 5]);
    assert_eq!(cache.render(0, 8, &mut [&mut long, &mut short], None), Err(UnitRangeError::Invalid));
    assert_eq!((long, short), ([0.0; 8], [0.0; 5]));
    // An uncached range without a source.
    let (mut left, mut right) = ([99.0f32; 8], [99.0f32; 8]);
    assert_eq!(cache.render(4, 8, &mut [&mut left, &mut right], None), Err(UnitRangeError::Invalid));
    assert_eq!((left, right), ([0.0; 8], [0.0; 8]));
    // None of this reached the child or disturbed what was retained.
    assert_eq!(child.processed.len(), calls);
    let (status, left, _) = child.render(0, 8, 8);
    assert_eq!((status, left), (OK, ramp(1, 8)));
    let stats = child.stats();
    assert_eq!((stats.requests, stats.successful_requests, stats.cache_hits), (17, 2, 1));
    assert_eq!((stats.last_error, stats.latched_error), (0, 0));
}

#[test]
fn reentry_from_prepare_and_span_is_rejected_as_busy() {
    let mut child = Child::new(2, 8, 16, 4, 4);
    child.reenter_from_prepare = true;
    child.reenter_from_span = true;
    let (status, left, _) = child.render(0, 8, 8);
    assert_eq!((status, left), (OK, ramp(1, 8)));
    // Once from prepare and once from each of the two span queries.
    assert_eq!(child.reentered, 3);
    // The flag is released again on every path out of a render.
    assert_eq!(child.stats().requests, 1);
    assert_eq!(child.cache.reset(), Ok(()));
    assert_eq!(child.stats(), UnitRangeStats::default());
}

/// A source that panics leaves the cache usable: the busy flag is released while unwinding.
#[test]
fn a_panicking_source_releases_the_cache() {
    struct Panics;
    impl UnitRangeSource for Panics {
        fn span(&mut self, _start: i64, _frames: u32) -> Result<ClockSpan, ClockQueryError> {
            panic!("no clock");
        }
        fn process(&mut self, _context: &ClockContext, _start: i64, _frames: u32, _output: &mut [&mut [f32]]) -> Result<(), NonZeroI32> {
            Ok(())
        }
    }
    let cache = UnitRangeCache::new(1, 8, 8, 8, 1).expect("a valid cache");
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut samples = [0.0f32; 8];
        let _ = cache.render(0, 8, &mut [&mut samples], Some(&mut Panics));
    }));
    assert!(attempt.is_err());
    assert_eq!(cache.stats().map(|stats| (stats.requests, stats.anchored)), Ok((1, 0)));
    assert_eq!(cache.reset(), Ok(()));
}

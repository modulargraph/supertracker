//! Port of the former C++ test program `unit_range_tests.cpp`: the bounded render cache for one hosted unit:
//! 4097/4096 overlap, retention, complete preflight, revision/epoch changes, failure recovery,
//! child-error latching, validation, concurrent rejection, real clock spans, zero allocations.
//!
//! The tests drive `tracker_engine::unit_range` through `engine_support_b::unit_range`, which
//! turns its results into the status codes the assertions compare.
//!
//! Dropped from the C++ program (C pointer mechanics, all in `creation_and_request_validation`):
//! null cache handles for render/reset/get_stats, a null stats pointer, a null output array and a
//! null channel pointer, and the error-buffer variants of a failing creation (8-byte truncation,
//! the 1-byte buffer and the null buffer with a capacity).
#![cfg(all(feature = "unit-range", feature = "clock"))]
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations};
use engine_support_b::unit_range::*;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tracker_engine::clock::*;
use tracker_engine::unit_range::*;

#[derive(Clone, Copy, Default)]
struct Call {
    frame: i64,
    frames: u32,
    revision: u64,
    epoch: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum BadSpan {
    None,
    Zero,
    TooLong,
    Coordinate,
    RateNaN,
    RateZero,
    NegativeOffset,
    HugeOffset,
    Boundary,
    TempoNaN,
    TempoZero,
    BeatNaN,
    BarStartNaN,
    BarEndNaN,
    Numerator,
    Denominator,
}

/// The callback side of the C++ `Fixture`: everything `prepare`/`span`/`process` read and write.
struct Probe {
    cache: Arc<Cache>,
    channels: u32,
    prepares: u64,
    queries: u64,
    processes: u64,
    events: u64,
    advanced: u64,
    prepared_start: i64,
    prepared_end: i64,
    planned_end: i64,
    processing_started: bool,
    tag_context: bool,
    require_prepare: bool,
    prepare_error: i32,
    span_error: i32,
    process_error: i32,
    fail_process: u64,
    bad: BadSpan,
    rate: f64,
    later_rate: f64,
    offset: i64,
    partition: i64,
    later_offset: i64,
    revision_boundary: i64,
    revision: u64,
    epoch: u64,
    new_revision: u64,
    new_epoch: u64,
    timeline: Option<Arc<ClockTimeline>>,
    query_trace: Vec<Call>,
    process_trace: Vec<Call>,
    query_trace_count: usize,
    process_trace_count: usize,
    reenter: bool,
    pause_process: Arc<AtomicBool>,
    process_entered: Arc<AtomicBool>,
    release_process: Arc<AtomicBool>,
}

impl Source for Probe {
    fn prepare(&mut self, start: i64, frames: u32) -> i32 {
        self.prepares += 1;
        self.prepared_start = start;
        self.prepared_end = start.wrapping_add(frames as i64);
        self.planned_end = start;
        self.processing_started = false;
        self.prepare_error
    }

    fn span(&mut self, start: i64, frames: u32, result: &mut ClockSpan) -> i32 {
        self.queries += 1;
        assert!(!self.processing_started); // The complete plan precedes every child call.
        if self.require_prepare {
            assert!(start >= self.prepared_start && start.wrapping_add(frames as i64) <= self.prepared_end);
            assert!(start == self.planned_end);
        }
        if self.span_error != CLOCK_OK {
            return self.span_error;
        }
        if let Some(timeline) = &self.timeline {
            match timeline.span(start, frames, self.offset) {
                Ok(span) => *result = span,
                Err(error) => return error.status(),
            }
        } else {
            *result = ClockSpan::default();
            result.frames = frames;
            if self.partition > 0 {
                let remainder = ((start % self.partition) + self.partition) % self.partition;
                result.frames = frames.min((self.partition - remainder) as u32);
            }
            if start < self.revision_boundary
                && (self.revision_boundary as u64).wrapping_sub(start as u64) < result.frames as u64
            {
                result.frames = (self.revision_boundary as u64).wrapping_sub(start as u64) as u32;
            }
            let later = start >= self.revision_boundary;
            let effective_offset = if later && self.later_offset >= 0 { self.later_offset } else { self.offset };
            let context = &mut result.context;
            context.graph_frame = start;
            context.content_graph_frame = start.wrapping_sub(effective_offset);
            context.song_frame = context.content_graph_frame;
            context.sample_rate = if later && self.later_rate > 0.0 { self.later_rate } else { self.rate };
            context.revision = if later { self.new_revision } else { self.revision };
            context.epoch = if later { self.new_epoch } else { self.epoch };
            context.beat = context.song_frame as f64 / 24000.0;
            context.bpm = 120.0;
            context.numerator = 4;
            context.denominator = 4;
            context.flags = CLOCK_PLAYING;
            context.next_boundary_frame = start.wrapping_add(result.frames as i64);
        }
        match self.bad {
            BadSpan::None => {}
            BadSpan::Zero => result.frames = 0,
            BadSpan::TooLong => result.frames = frames + 1,
            BadSpan::Coordinate => result.context.graph_frame += 1,
            BadSpan::RateNaN => result.context.sample_rate = f64::NAN,
            BadSpan::RateZero => result.context.sample_rate = 0.0,
            BadSpan::NegativeOffset => result.context.content_graph_frame = start + 1,
            BadSpan::HugeOffset => result.context.content_graph_frame = start - (1i64 << 30) - 1,
            BadSpan::Boundary => result.context.next_boundary_frame = start + result.frames as i64 - 1,
            BadSpan::TempoNaN => result.context.bpm = f64::INFINITY,
            BadSpan::TempoZero => result.context.bpm = 0.0,
            BadSpan::BeatNaN => result.context.beat = f64::NAN,
            BadSpan::BarStartNaN => result.context.bar_start_beat = f64::NAN,
            BadSpan::BarEndNaN => result.context.bar_end_beat = f64::INFINITY,
            BadSpan::Numerator => result.context.numerator = 0,
            BadSpan::Denominator => result.context.denominator = 3,
        }
        if self.query_trace_count < self.query_trace.len() {
            self.query_trace[self.query_trace_count] = Call {
                frame: start,
                frames: result.frames,
                revision: result.context.revision,
                epoch: result.context.epoch,
            };
            self.query_trace_count += 1;
        }
        self.planned_end = start.wrapping_add(result.frames as i64);
        CLOCK_OK
    }

    fn process(&mut self, context: &ClockContext, start: i64, frames: u32, output: &mut [&mut [f32]]) -> i32 {
        if self.require_prepare {
            assert!(self.planned_end == self.prepared_end);
        }
        assert!(context.graph_frame == start);
        self.processing_started = true;
        self.processes += 1;
        self.events += 1; // Models per-child MIDI/parameter dispatch.
        if self.process_trace_count < self.process_trace.len() {
            self.process_trace[self.process_trace_count] =
                Call { frame: start, frames, revision: context.revision, epoch: context.epoch };
            self.process_trace_count += 1;
        }
        if self.reenter {
            self.reenter = false;
            let (mut l, mut r) = ([1.0f32; 4], [1.0f32; 4]);
            {
                let mut nested: [&mut [f32]; 2] = [&mut l, &mut r];
                assert!(
                    self.cache.render(
                        start,
                        4,
                        &mut nested[..self.channels as usize],
                        &mut NeverCalled,
                        Callbacks::ALL
                    ) == ST_UNIT_RANGE_BUSY
                );
            }
            assert!(l.iter().all(|&value| value == 0.0));
            if self.channels == 2 {
                assert!(r.iter().all(|&value| value == 0.0));
            }
            let mut untouched = UnitRangeStats { requests: 123, ..UnitRangeStats::default() };
            assert!(self.cache.stats_into(&mut untouched) == ST_UNIT_RANGE_BUSY && untouched.requests == 123);
            assert!(self.cache.reset() == ST_UNIT_RANGE_BUSY);
        }
        if self.pause_process.load(Ordering::Relaxed) {
            self.process_entered.store(true, Ordering::Release);
            // Deliberate test-only stalled processor, so another thread can probe BUSY.
            while !self.release_process.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        }
        let tag = if self.tag_context { (context.revision * 1000 + context.epoch * 10) as f32 } else { 0.0 };
        for index in 0..frames as usize {
            self.advanced += 1;
            let value = self.advanced as f32 + tag;
            output[0][index] = value;
            if self.channels == 2 {
                output[1][index] = -2.0 * value;
            }
        }
        if self.fail_process == self.processes {
            self.process_error
        } else {
            0
        }
    }
}

/// The C++ `Fixture`: a cache, its probe callbacks and caller-side output buffers with sixteen
/// guard frames (99) behind the largest request.
struct Fixture {
    cache: Arc<Cache>,
    left: Vec<f32>,
    right: Vec<f32>,
    probe: Probe,
}

impl Deref for Fixture {
    type Target = Probe;
    fn deref(&self) -> &Probe {
        &self.probe
    }
}

impl DerefMut for Fixture {
    fn deref_mut(&mut self) -> &mut Probe {
        &mut self.probe
    }
}

impl Fixture {
    /// C++ `Fixture(request = 16384, history = 32768, child = 4096, spans = 64, channelCount = 2)`.
    fn with(request: u32, history: u32, child: u32, spans: u32, channel_count: u32) -> Fixture {
        let cache = Cache::create(channel_count, request, history, child, spans);
        assert!(cache.is_ok());
        let cache = Arc::new(cache.unwrap());
        let probe = Probe {
            cache: cache.clone(),
            channels: channel_count,
            prepares: 0,
            queries: 0,
            processes: 0,
            events: 0,
            advanced: 0,
            prepared_start: 0,
            prepared_end: 0,
            planned_end: 0,
            processing_started: false,
            tag_context: false,
            require_prepare: true,
            prepare_error: 0,
            span_error: CLOCK_OK,
            process_error: -12345,
            fail_process: 0,
            bad: BadSpan::None,
            rate: 48000.0,
            later_rate: 0.0,
            offset: 0,
            partition: 0,
            later_offset: -1,
            revision_boundary: i64::MAX,
            revision: 1,
            epoch: 1,
            new_revision: 2,
            new_epoch: 2,
            timeline: None,
            query_trace: vec![Call::default(); 1024],
            process_trace: vec![Call::default(); 1024],
            query_trace_count: 0,
            process_trace_count: 0,
            reenter: false,
            pause_process: Arc::new(AtomicBool::new(false)),
            process_entered: Arc::new(AtomicBool::new(false)),
            release_process: Arc::new(AtomicBool::new(false)),
        };
        Fixture { cache, left: vec![99.0; request as usize + 16], right: vec![99.0; request as usize + 16], probe }
    }

    /// `Fixture fixture;`
    fn new() -> Fixture {
        Fixture::with(16384, 32768, 4096, 64, 2)
    }

    /// `Fixture fixture(request, history, child, spans);`: stereo.
    fn stereo(request: u32, history: u32, child: u32, spans: u32) -> Fixture {
        Fixture::with(request, history, child, spans, 2)
    }

    fn render_with(&mut self, start: i64, frames: u32, callbacks: Callbacks) -> i32 {
        let Fixture { cache, left, right, probe } = self;
        let channels = probe.channels as usize;
        let mut output: [&mut [f32]; 2] = [left, right];
        cache.render(start, frames, &mut output[..channels], probe, callbacks)
    }

    fn render(&mut self, start: i64, frames: u32) -> i32 {
        self.left.fill(99.0);
        self.right.fill(99.0);
        realtime(|| self.render_with(start, frames, Callbacks::ALL))
    }

    /// A request without any callback (served from retained audio only).
    fn render_cached(&mut self, start: i64, frames: u32) -> i32 {
        let Fixture { cache, left, right, probe } = self;
        let channels = probe.channels as usize;
        let mut output: [&mut [f32]; 2] = [left, right];
        cache.render_cached(start, frames, &mut output[..channels])
    }

    #[track_caller]
    fn stats(&self) -> UnitRangeStats {
        let mut value = UnitRangeStats::default();
        assert!(self.cache.stats_into(&mut value) == 0);
        value
    }

    #[track_caller]
    fn expect_sequence_tagged(&self, frames: u32, first: u64, tag: f32) {
        for index in 0..frames as usize {
            let expected = (first + index as u64) as f32 + tag;
            assert!(self.left[index] == expected);
            if self.channels == 2 {
                assert!(self.right[index] == -2.0 * expected);
            }
        }
        assert!(self.left[frames as usize] == 99.0);
        if self.channels == 2 {
            assert!(self.right[frames as usize] == 99.0);
        }
    }

    #[track_caller]
    fn expect_sequence(&self, frames: u32, first: u64) {
        self.expect_sequence_tagged(frames, first, 0.0);
    }

    #[track_caller]
    fn expect_silent(&self, frames: u32) {
        for index in 0..frames as usize {
            assert!(self.left[index] == 0.0);
            if self.channels == 2 {
                assert!(self.right[index] == 0.0);
            }
        }
    }
}

/// The program's final `assert(renderAllocations.load()==0)`.
fn zero_render_allocations() {
    assert!(render_allocations() == 0);
}

#[test]
fn overlap_4097_and_cache_hits() {
    let mut fixture = Fixture::new();
    assert!(fixture.render(0, 4097) == 0);
    fixture.expect_sequence(4097, 1);
    assert!(fixture.processes == 2 && fixture.process_trace[0].frames == 4096 && fixture.process_trace[1].frames == 1);
    assert!(fixture.render(4096, 4096) == 0);
    fixture.expect_sequence(4096, 4097);
    assert!(fixture.advanced == 8192 && fixture.prepared_start == 4097 && fixture.prepared_end == 8192);
    assert!(fixture.process_trace[2].frame == 4097 && fixture.process_trace[2].frames == 4095);
    let (calls, queries, prepares) = (fixture.processes, fixture.queries, fixture.prepares);
    assert!(fixture.render(0, 8192) == 0);
    fixture.expect_sequence(8192, 1);
    assert!(fixture.render(4096, 4096) == 0);
    fixture.expect_sequence(4096, 4097);
    assert!(fixture.render(4000, 100) == 0);
    fixture.expect_sequence(100, 4001);
    assert!(fixture.processes == calls && fixture.queries == queries && fixture.prepares == prepares);
    let stats = fixture.stats();
    assert!(stats.processed_frames == 8192 && stats.prepared_frames == 8192 && stats.cache_hits == 3);
    assert!(stats.retained_start_frame == 0 && stats.rendered_end_frame == 8192 && stats.retained_frames == 8192);
    // A caller can satisfy a retained request after all clock/processor callbacks disappear.
    assert!(fixture.render_cached(0, 10) == 0);
    for i in 0..10 {
        assert!(fixture.left[i] == (i + 1) as f32);
    }
    zero_render_allocations();
}

#[test]
fn ring_wrap_and_retention() {
    let mut fixture = Fixture::stereo(8, 8, 4, 4);
    assert!(fixture.render(100, 8) == 0);
    fixture.expect_sequence(8, 1);
    assert!(fixture.render(104, 8) == 0);
    fixture.expect_sequence(8, 5);
    assert!(fixture.render(110, 8) == 0);
    fixture.expect_sequence(8, 11);
    assert!(fixture.render(114, 8) == 0);
    fixture.expect_sequence(8, 15);
    let count = fixture.processes;
    assert!(fixture.render(113, 1) == ST_UNIT_RANGE_EXPIRED);
    fixture.expect_silent(1);
    assert!(fixture.render(123, 1) == ST_UNIT_RANGE_GAP);
    fixture.expect_silent(1);
    assert!(fixture.processes == count);
    assert!(fixture.render(114, 8) == 0);
    fixture.expect_sequence(8, 15);
    assert!(fixture.render(121, 8) == 0);
    fixture.expect_sequence(8, 22);
    let stats = fixture.stats();
    assert!(stats.retained_start_frame == 121 && stats.rendered_end_frame == 129);
    assert!(stats.retained_frames == 8 && stats.processed_frames == 29 && stats.cache_hits == 1);
    let mut larger = Fixture::with(8, 13, 3, 8, 1);
    let mut frame: i64 = -100;
    while frame < 100 {
        assert!(larger.render(frame, 7) == 0);
        larger.expect_sequence(7, (frame + 101) as u64);
        assert!(larger.render(frame + 1, 4) == 0);
        larger.expect_sequence(4, (frame + 102) as u64);
        frame += 7;
    }
    assert!(larger.stats().retained_frames == 13);
    zero_render_allocations();
}

#[test]
fn full_preflight_before_events() {
    let mut fixture = Fixture::stereo(32, 64, 9, 16);
    fixture.partition = 7;
    assert!(fixture.render(3, 32) == 0);
    fixture.expect_sequence(32, 1);
    assert!(fixture.queries == 5 && fixture.processes == 5 && fixture.events == 5);
    for index in 0..fixture.process_trace_count {
        let call = fixture.process_trace[index];
        assert!(call.frame == fixture.query_trace[index].frame && call.frames == fixture.query_trace[index].frames);
        assert!(call.frames <= 7 && call.frame / 7 == (call.frame + call.frames as i64 - 1) / 7);
    }
    let mut limited = Fixture::stereo(16, 32, 8, 2);
    assert!(limited.render(0, 8) == 0);
    limited.expect_sequence(8, 1);
    limited.partition = 1;
    assert!(limited.render(4, 12) == ST_UNIT_RANGE_SPAN_LIMIT);
    limited.expect_silent(12);
    assert!(limited.processes == 1 && limited.events == 1 && limited.advanced == 8);
    let stats = limited.stats();
    assert!(stats.rendered_end_frame == 8 && stats.latched_error == 0 && stats.prepared_frames == 16);
    assert!(limited.render(0, 8) == 0);
    limited.expect_sequence(8, 1);
    limited.partition = 0;
    assert!(limited.render(4, 12) == 0);
    limited.expect_sequence(12, 5);
    assert!(limited.processes == 2 && limited.events == 2 && limited.prepared_start == 8 && limited.prepared_end == 16);
    zero_render_allocations();
}

#[test]
fn clock_replacement_and_epoch_boundaries() {
    let mut fixture = Fixture::stereo(32, 64, 8, 16);
    fixture.tag_context = true;
    fixture.revision = 7;
    fixture.epoch = 11;
    fixture.revision_boundary = 13;
    fixture.new_revision = 8;
    fixture.new_epoch = 12;
    assert!(fixture.render(0, 24) == 0);
    for index in 0..24 {
        let expected = (index + 1) as f32 + (if index < 13 { 7110.0f32 } else { 8120.0f32 });
        assert!(fixture.left[index] == expected && fixture.right[index] == -2.0 * expected);
    }
    assert!(fixture.process_trace[0].revision == 7 && fixture.process_trace[1].frames == 5);
    assert!(
        fixture.process_trace[2].frame == 13
            && fixture.process_trace[2].revision == 8
            && fixture.process_trace[2].epoch == 12
    );
    let (queries, processes) = (fixture.queries, fixture.processes);
    // Replace the provider's map/origin entirely. Retained samples keep their old interpretation.
    fixture.revision_boundary = 0;
    fixture.new_revision = 9;
    fixture.new_epoch = 99;
    assert!(fixture.render(8, 16) == 0);
    for index in 0..16 {
        assert!(fixture.left[index] == (index + 9) as f32 + (if index < 5 { 7110.0f32 } else { 8120.0f32 }));
    }
    assert!(fixture.queries == queries && fixture.processes == processes);
    assert!(fixture.render(20, 12) == 0);
    for index in 0..12 {
        assert!(fixture.left[index] == (index + 21) as f32 + (if index < 4 { 8120.0f32 } else { 9990.0f32 }));
    }
    assert!(
        fixture.prepared_start == 24
            && fixture.process_trace[processes as usize].revision == 9
            && fixture.process_trace[processes as usize].epoch == 99
    );
    let stats = fixture.stats();
    assert!(stats.revision == 9 && stats.epoch == 99 && stats.processed_frames == 32);
    zero_render_allocations();
}

#[test]
fn prepare_and_clock_failures_retry() {
    let mut fixture = Fixture::stereo(16, 32, 4, 16);
    assert!(fixture.render(0, 8) == 0);
    fixture.expect_sequence(8, 1);
    let (queries, processes) = (fixture.queries, fixture.processes);
    fixture.prepare_error = -901;
    assert!(fixture.render(4, 12) == -901);
    fixture.expect_silent(12);
    assert!(fixture.queries == queries && fixture.processes == processes && fixture.stats().latched_error == 0);
    fixture.prepare_error = 0;
    for bad in [
        BadSpan::Zero,
        BadSpan::TooLong,
        BadSpan::Coordinate,
        BadSpan::RateNaN,
        BadSpan::RateZero,
        BadSpan::NegativeOffset,
        BadSpan::HugeOffset,
        BadSpan::Boundary,
        BadSpan::TempoNaN,
        BadSpan::TempoZero,
        BadSpan::BeatNaN,
        BadSpan::BarStartNaN,
        BadSpan::BarEndNaN,
        BadSpan::Numerator,
        BadSpan::Denominator,
    ] {
        fixture.bad = bad;
        let expected = if bad == BadSpan::Zero { ST_UNIT_RANGE_DOMAIN_END } else { ST_UNIT_RANGE_CONTEXT_MISMATCH };
        assert!(fixture.render(4, 12) == expected);
        fixture.expect_silent(12);
        assert!(fixture.processes == processes && fixture.events == processes);
        assert!(fixture.stats().rendered_end_frame == 8 && fixture.stats().latched_error == 0);
        assert!(fixture.render(0, 8) == 0);
        fixture.expect_sequence(8, 1);
    }
    fixture.bad = BadSpan::None;
    fixture.span_error = CLOCK_INVALID;
    assert!(fixture.render(8, 8) == ST_UNIT_RANGE_CLOCK_ERROR);
    fixture.expect_silent(8);
    fixture.span_error = CLOCK_OUTSIDE_DOMAIN;
    assert!(fixture.render(8, 8) == ST_UNIT_RANGE_DOMAIN_END);
    fixture.expect_silent(8);
    fixture.span_error = CLOCK_OK;
    fixture.rate = 44100.0;
    assert!(fixture.render(8, 8) == ST_UNIT_RANGE_CONTEXT_MISMATCH);
    fixture.expect_silent(8);
    fixture.rate = 48000.0;
    fixture.offset = 64;
    assert!(fixture.render(8, 8) == ST_UNIT_RANGE_CONTEXT_MISMATCH);
    fixture.expect_silent(8);
    fixture.offset = 0;
    assert!(fixture.render(4, 12) == 0);
    fixture.expect_sequence(12, 5);
    assert!(fixture.advanced == 16);
    assert!(fixture.cache.reset() == 0);
    fixture.rate = 44100.0;
    fixture.offset = 64;
    fixture.advanced = 0;
    assert!(fixture.render(100, 16) == 0);
    fixture.expect_sequence(16, 1);
    // The first request also rejects an inconsistent format in a later span,
    // before committing an anchor or delivering events to any child.
    let mut first = Fixture::stereo(16, 32, 8, 4);
    first.revision_boundary = 8;
    first.later_rate = 44100.0;
    assert!(first.render(0, 16) == ST_UNIT_RANGE_CONTEXT_MISMATCH);
    first.expect_silent(16);
    assert!(first.queries == 2 && first.processes == 0 && first.events == 0 && first.stats().anchored == 0);
    first.later_rate = 0.0;
    first.later_offset = 64;
    assert!(first.render(0, 16) == ST_UNIT_RANGE_CONTEXT_MISMATCH);
    first.expect_silent(16);
    assert!(first.processes == 0 && first.events == 0 && first.stats().anchored == 0);
    first.later_offset = -1;
    assert!(first.render(0, 16) == 0);
    first.expect_sequence(16, 1);
    zero_render_allocations();
}

#[test]
fn child_failure_latches_and_reset() {
    let mut fixture = Fixture::stereo(16, 32, 4, 16);
    assert!(fixture.render(0, 4) == 0);
    fixture.expect_sequence(4, 1);
    fixture.fail_process = 3;
    assert!(fixture.render(2, 14) == fixture.process_error);
    fixture.expect_silent(14);
    let mut stats = fixture.stats();
    assert!(stats.process_calls == 3 && stats.processed_frames == 8 && stats.rendered_end_frame == 4);
    assert!(stats.retained_frames == 4 && stats.latched_error == fixture.process_error && fixture.advanced == 12);
    let (calls, queries, prepares) = (fixture.processes, fixture.queries, fixture.prepares);
    assert!(fixture.render(0, 4) == fixture.process_error);
    fixture.expect_silent(4);
    assert!(fixture.render(4, 12) == fixture.process_error);
    fixture.expect_silent(12);
    assert!(fixture.processes == calls && fixture.queries == queries && fixture.prepares == prepares);
    assert!(fixture.cache.reset() == 0);
    stats = fixture.stats();
    assert!(stats.anchored == 0 && stats.latched_error == 0 && stats.requests == 0 && stats.processed_frames == 0);
    fixture.advanced = 0;
    fixture.fail_process = 0;
    assert!(fixture.render(-23, 16) == 0);
    fixture.expect_sequence(16, 1);
    assert!(fixture.stats().retained_start_frame == -23 && fixture.stats().rendered_end_frame == -7);
    zero_render_allocations();
}

#[test]
fn creation_and_request_validation() {
    // C++ `invalid`: creation fails with a non-empty error text (the 8-byte buffer's truncation
    // check is dropped).
    let invalid = |channels: u32, request: u32, history: u32, child: u32, spans: u32| {
        let result = Cache::create(channels, request, history, child, spans);
        assert!(result.is_err());
        assert!(result.is_err_and(|error| !error.is_empty()));
    };
    invalid(0, 8, 8, 8, 1);
    invalid(3, 8, 8, 8, 1);
    invalid(1, 0, 8, 8, 1);
    invalid(1, 65537, 65537, 8, 1);
    invalid(1, 8, 7, 8, 1);
    invalid(1, 8, 131073, 8, 1);
    invalid(1, 8, 8, 0, 1);
    invalid(1, 8, 8, 9, 1);
    invalid(1, 8, 8, 8, 0);
    invalid(1, 8, 8, 8, 257);
    invalid(1, 16, 16, 4, 3);
    invalid(1, 65536, 65536, 1, 256);
    // (C++: the same failing creation with a 1-byte error buffer and with a null buffer.)
    assert!(Cache::create(0, 8, 8, 8, 1).is_err());
    let mut fixture = Fixture::stereo(8, 16, 8, 2);
    // (C++: null cache for render/reset/get_stats and a null stats pointer are INVALID.)
    assert!(fixture.render(0, 0) == ST_UNIT_RANGE_INVALID && fixture.left[0] == 99.0);
    assert!(fixture.render(0, 9) == ST_UNIT_RANGE_OVERSIZE);
    fixture.expect_silent(8);
    assert!(fixture.left[8] == 99.0 && fixture.right[8] == 99.0);
    assert!(fixture.render(i64::MAX - 3, 8) == ST_UNIT_RANGE_OVERFLOW);
    fixture.expect_silent(8);
    // (C++: a null output array and a null channel pointer are INVALID.)
    assert!(fixture.render_with(0, 8, Callbacks::PROCESS_ONLY) == ST_UNIT_RANGE_INVALID);
    assert!(fixture.render_with(0, 8, Callbacks::SPAN_ONLY) == ST_UNIT_RANGE_INVALID);
    assert!(fixture.prepares == 0 && fixture.queries == 0 && fixture.processes == 0);
    fixture.require_prepare = false;
    assert!(fixture.render_with(0, 8, Callbacks::WITHOUT_PREPARE) == 0);
    assert!(fixture.prepares == 0 && fixture.advanced == 8);
    // Maximal allowed storage and split count, plus a nonzero initial graph anchor.
    let mut maximum = Fixture::with(65536, 131072, 256, 256, 1);
    assert!(maximum.render(123456, 65536) == 0);
    maximum.expect_sequence(65536, 1);
    assert!(maximum.processes == 256);
    let mut extreme = Fixture::with(8, 8, 8, 1, 1);
    assert!(extreme.render(i64::MIN, 8) == 0);
    extreme.expect_sequence(8, 1);
    assert!(extreme.render(i64::MIN + 4, 4) == 0);
    extreme.expect_sequence(4, 5);
    assert!(extreme.cache.reset() == 0);
    extreme.advanced = 0;
    assert!(extreme.render(i64::MAX - 8, 8) == 0);
    extreme.expect_sequence(8, 1);
    assert!(extreme.render(i64::MAX - 4, 4) == 0);
    extreme.expect_sequence(4, 5);
    zero_render_allocations();
}

#[test]
fn reentrancy_and_concurrent_rejection() {
    let mut fixture = Fixture::stereo(16, 32, 16, 2);
    fixture.reenter = true;
    assert!(fixture.render(0, 16) == 0);
    fixture.expect_sequence(16, 1);
    fixture.pause_process.store(true, Ordering::SeqCst);
    let (cache, entered, release) =
        (fixture.cache.clone(), fixture.process_entered.clone(), fixture.release_process.clone());
    let elapsed = std::thread::scope(|scope| {
        let fixture = &mut fixture;
        let renderer = scope.spawn(move || {
            assert!(fixture.render(16, 16) == 0);
            render_allocations()
        });
        while !entered.load(Ordering::Acquire) && !renderer.is_finished() {
            std::thread::yield_now();
        }
        let (mut left, mut right) = ([0.0f32; 16], [0.0f32; 16]);
        let began = Instant::now();
        for _attempt in 0..10000 {
            left.fill(99.0);
            right.fill(99.0);
            {
                let mut output: [&mut [f32]; 2] = [&mut left, &mut right];
                assert!(cache.render_cached(16, 16, &mut output) == ST_UNIT_RANGE_BUSY);
            }
            assert!(left.iter().all(|&value| value == 0.0));
            assert!(right.iter().all(|&value| value == 0.0));
            let mut stats = UnitRangeStats { requests: 999, ..UnitRangeStats::default() };
            assert!(cache.stats_into(&mut stats) == ST_UNIT_RANGE_BUSY && stats.requests == 999);
            assert!(cache.reset() == ST_UNIT_RANGE_BUSY);
        }
        let elapsed = began.elapsed().as_secs_f64();
        release.store(true, Ordering::Release);
        // The stalled render ran as its thread's "render in progress" section.
        assert!(renderer.join().expect("renderer thread") == 0);
        elapsed
    });
    assert!(elapsed < 2.0);
    fixture.expect_sequence(16, 17);
    let stats = fixture.stats();
    assert!(stats.requests == 2 && stats.processed_frames == 32 && stats.latched_error == 0);
    println!("  Concurrent rejection: 10,000 attempts in {elapsed:.4} seconds while child was stalled.");
    zero_render_allocations();
}

#[test]
fn actual_clock_tempo_meter_and_latency() {
    let tempos = [
        ClockTempoEvent { beat: 0.0, bpm: 120.0 },
        ClockTempoEvent { beat: 0.25, bpm: 60.0 },
        ClockTempoEvent { beat: 0.3, bpm: 180.0 },
    ];
    let meters = [
        ClockMeterEvent { beat: 0.0, numerator: 4, denominator: 4 },
        ClockMeterEvent { beat: 0.25, numerator: 5, denominator: 8 },
        ClockMeterEvent { beat: 0.3, numerator: 7, denominator: 8 },
    ];
    let map = ClockMap::new(48000.0, &tempos, &meters, 55);
    assert!(map.is_ok());
    let map = map.unwrap();
    let origin = ClockOrigin {
        graph_origin_frame: 0,
        start_beat: 0.0,
        playing: 1,
        loop_enabled: 0,
        loop_start_beat: 0.0,
        loop_end_beat: 0.0,
        epoch: 88,
    };
    let timeline = ClockTimeline::new(&map, origin);
    assert!(timeline.is_ok());
    let timeline = Arc::new(timeline.unwrap());
    drop(map);
    let mut fixture = Fixture::stereo(16384, 32768, 4096, 64);
    fixture.timeline = Some(timeline.clone());
    fixture.offset = 257;
    assert!(fixture.render(0, 16384) == 0);
    fixture.expect_sequence(16384, 1);
    let (mut preroll, mut tempo, mut meter) = (false, false, false);
    for index in 0..fixture.process_trace_count {
        let call = fixture.process_trace[index];
        let first = timeline.context(call.frame, 257);
        assert!(first.is_ok());
        let last = timeline.context(call.frame + call.frames as i64 - 1, 257);
        assert!(last.is_ok());
        let (first, last) = (first.unwrap(), last.unwrap());
        assert!(first.bpm == last.bpm && first.numerator == last.numerator && first.denominator == last.denominator);
        assert!(first.bar_start_beat == last.bar_start_beat && first.content_graph_frame == call.frame - 257);
        assert!(call.revision == 55 && call.epoch == 88 && call.frames <= 4096);
        preroll |= (first.flags & CLOCK_PREROLL) != 0;
        tempo |= first.bpm == 60.0;
        meter |= first.numerator == 7;
    }
    assert!(preroll && tempo && meter);
    let events = fixture.events;
    assert!(fixture.render(6000, 8000) == 0);
    fixture.expect_sequence(8000, 6001);
    assert!(fixture.events == events);
    fixture.timeline = None;
    zero_render_allocations();
}

#[test]
fn long_run_no_render_allocations() {
    let mut fixture = Fixture::stereo(16384, 32768, 4096, 64);
    fixture.partition = 6000;
    let mut random: u64 = 192874312;
    let mut frontier: i64 = 0;
    for _request in 0..2000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let extra = (random % 8192) as u32 + 1;
        let overlap = frontier.min(((random >> 32) % 4097) as i64) as u32;
        let start = frontier - overlap as i64;
        assert!(fixture.render(start, overlap + extra) == 0);
        fixture.expect_sequence(overlap + extra, (start + 1) as u64);
        frontier += extra as i64;
        assert!(fixture.advanced == frontier as u64);
        assert!(fixture.render(frontier - 1, 1) == 0);
        fixture.expect_sequence(1, frontier as u64);
    }
    assert!(render_allocations() == 0);
    println!("  Stateful overlap stress: {} frames, 4,000 requests, zero render allocations.", fixture.advanced);
    zero_render_allocations();
}

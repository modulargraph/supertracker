//! The unit range cache as `tests/unit_range.rs` drives it: `tracker_engine::unit_range` behind
//! the status codes and the optional callbacks the test program was written against.
//!
//! * [`Cache`] returns each result as its numeric code (0 for success), which is what the tests
//!   compare; the codes are [`UnitRangeError::code`].
//! * [`Source`] is the tests' child: `prepare`/`span`/`process` returning status codes. A render
//!   passes it whole or in part ([`Callbacks`]): without `prepare` the cache's default (nothing
//!   to prepare) applies, and without `span` or `process` there is no usable source at all.

use std::num::NonZeroI32;
use tracker_engine::clock::{ClockContext, ClockQueryError, ClockSpan, CLOCK_OK, CLOCK_OUTSIDE_DOMAIN};
use tracker_engine::unit_range::*;

pub const ST_UNIT_RANGE_OK: i32 = 0;
pub const ST_UNIT_RANGE_INVALID: i32 = UnitRangeError::Invalid.code();
pub const ST_UNIT_RANGE_OVERSIZE: i32 = UnitRangeError::Oversize.code();
pub const ST_UNIT_RANGE_OVERFLOW: i32 = UnitRangeError::Overflow.code();
pub const ST_UNIT_RANGE_GAP: i32 = UnitRangeError::Gap.code();
pub const ST_UNIT_RANGE_EXPIRED: i32 = UnitRangeError::Expired.code();
pub const ST_UNIT_RANGE_BUSY: i32 = UnitRangeError::Busy.code();
pub const ST_UNIT_RANGE_CLOCK_ERROR: i32 = UnitRangeError::ClockError.code();
pub const ST_UNIT_RANGE_DOMAIN_END: i32 = UnitRangeError::DomainEnd.code();
pub const ST_UNIT_RANGE_CONTEXT_MISMATCH: i32 = UnitRangeError::ContextMismatch.code();
pub const ST_UNIT_RANGE_SPAN_LIMIT: i32 = UnitRangeError::SpanLimit.code();

/// The child of a cache, with every result as a status code.
pub trait Source {
    /// Extend the upstream caches for the uncached suffix; nonzero fails the request.
    fn prepare(&mut self, graph_frame: i64, frames: u32) -> i32;
    /// The clock span at `graph_frame` (at most `frames` long); `CLOCK_OK` or a clock error.
    fn span(&mut self, graph_frame: i64, frames: u32, span: &mut ClockSpan) -> i32;
    /// Render `frames` frames of every channel; nonzero fails the request and is latched.
    fn process(&mut self, context: &ClockContext, graph_frame: i64, frames: u32, output: &mut [&mut [f32]]) -> i32;
}

/// Which parts of a [`Source`] a render call passes.
#[derive(Clone, Copy)]
pub struct Callbacks {
    pub prepare: bool,
    pub span: bool,
    pub process: bool,
}

impl Callbacks {
    pub const ALL: Callbacks = Callbacks { prepare: true, span: true, process: true };
    pub const WITHOUT_PREPARE: Callbacks = Callbacks { prepare: false, span: true, process: true };
    pub const PROCESS_ONLY: Callbacks = Callbacks { prepare: false, span: false, process: true };
    pub const SPAN_ONLY: Callbacks = Callbacks { prepare: false, span: true, process: false };
}

/// An owned cache.
pub struct Cache {
    cache: UnitRangeCache,
}

/// A [`Source`] as the cache's `UnitRangeSource`.
struct Adapter<'a> {
    source: &'a mut dyn Source,
    prepare: bool,
}

fn status(code: i32) -> Result<(), NonZeroI32> {
    NonZeroI32::new(code).map_or(Ok(()), Err)
}

impl UnitRangeSource for Adapter<'_> {
    fn prepare(&mut self, graph_frame: i64, frames: u32) -> Result<(), NonZeroI32> {
        if self.prepare {
            status(self.source.prepare(graph_frame, frames))
        } else {
            Ok(())
        }
    }

    fn span(&mut self, graph_frame: i64, frames: u32) -> Result<ClockSpan, ClockQueryError> {
        let mut span = ClockSpan::default();
        match self.source.span(graph_frame, frames, &mut span) {
            CLOCK_OK => Ok(span),
            CLOCK_OUTSIDE_DOMAIN => Err(ClockQueryError::OutsideDomain),
            _ => Err(ClockQueryError::Invalid),
        }
    }

    fn process(&mut self, context: &ClockContext, graph_frame: i64, frames: u32, output: &mut [&mut [f32]]) -> Result<(), NonZeroI32> {
        status(self.source.process(context, graph_frame, frames, output))
    }
}

fn code(result: Result<(), UnitRangeError>) -> i32 {
    result.map_or_else(UnitRangeError::code, |()| ST_UNIT_RANGE_OK)
}

impl Cache {
    /// A new cache, or the error text.
    pub fn create(
        channels: u32,
        maximum_request_frames: u32,
        history_frames: u32,
        maximum_child_frames: u32,
        maximum_spans: u32,
    ) -> Result<Cache, String> {
        UnitRangeCache::new(channels, maximum_request_frames, history_frames, maximum_child_frames, maximum_spans)
            .map(|cache| Cache { cache })
            .map_err(|error| error.to_string())
    }

    pub fn reset(&self) -> i32 {
        code(self.cache.reset())
    }

    /// The status; `stats` is written only on success.
    pub fn stats_into(&self, stats: &mut UnitRangeStats) -> i32 {
        match self.cache.stats() {
            Ok(value) => {
                *stats = value;
                ST_UNIT_RANGE_OK
            }
            Err(error) => error.code(),
        }
    }

    /// A request that can only be served from retained audio.
    pub fn render_cached(&self, graph_frame: i64, frames: u32, output: &mut [&mut [f32]]) -> i32 {
        code(self.cache.render(graph_frame, frames, output, None))
    }

    /// A request with `source` as the parts named in `callbacks`.
    pub fn render(
        &self,
        graph_frame: i64,
        frames: u32,
        output: &mut [&mut [f32]],
        source: &mut dyn Source,
        callbacks: Callbacks,
    ) -> i32 {
        if callbacks.span && callbacks.process {
            let mut adapter = Adapter { source, prepare: callbacks.prepare };
            code(self.cache.render(graph_frame, frames, output, Some(&mut adapter)))
        } else {
            code(self.cache.render(graph_frame, frames, output, None))
        }
    }
}

/// A source for calls that must be rejected before the source is used.
pub struct NeverCalled;

impl Source for NeverCalled {
    fn prepare(&mut self, _graph_frame: i64, _frames: u32) -> i32 {
        panic!("prepare called on a request that must be rejected first");
    }
    fn span(&mut self, _graph_frame: i64, _frames: u32, _span: &mut ClockSpan) -> i32 {
        panic!("span called on a request that must be rejected first");
    }
    fn process(&mut self, _context: &ClockContext, _graph_frame: i64, _frames: u32, _output: &mut [&mut [f32]]) -> i32 {
        panic!("process called on a request that must be rejected first");
    }
}

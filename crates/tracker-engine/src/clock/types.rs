//! The plain data of the clock: events, origin, context, span and their constants.
//!
//! This file has no dependencies so that `unit_range` can use the types without the clock itself.

// Status codes of clock queries, for callers that keep a status. `CLOCK_OK` is 1, not 0.
pub const CLOCK_OK: i32 = 1;
pub const CLOCK_INVALID: i32 = 0;
pub const CLOCK_OUTSIDE_DOMAIN: i32 = -1;

// `ClockContext::flags`.
pub const CLOCK_PLAYING: i32 = 1;
pub const CLOCK_LOOPING: i32 = 2;
pub const CLOCK_PREROLL: i32 = 4;
pub const CLOCK_DOMAIN_END: i32 = 8;
pub const CLOCK_RECORDING: i32 = 16;
pub const CLOCK_GRID_PROJECTION: i32 = 32;

// `ClockContext::next_boundary_kinds`.
pub const CLOCK_BOUNDARY_TEMPO: u32 = 1;
pub const CLOCK_BOUNDARY_METER: u32 = 2;
pub const CLOCK_BOUNDARY_BAR: u32 = 4;
pub const CLOCK_BOUNDARY_LOOP: u32 = 8;
pub const CLOCK_BOUNDARY_ORIGIN: u32 = 16;
pub const CLOCK_BOUNDARY_DOMAIN: u32 = 32;

/// Most tempo events, and most meter events, in one map.
pub const CLOCK_MAX_EVENTS: u32 = 1024;
/// Most frames in one span request.
pub const CLOCK_MAX_SPAN: u32 = 65536;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// A tempo change: `bpm` from quarter beat `beat` on.
pub struct ClockTempoEvent {
    pub beat: f64,
    pub bpm: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// A meter change at quarter beat `beat` (a bar line).
pub struct ClockMeterEvent {
    pub beat: f64,
    pub numerator: i32,
    pub denominator: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// Where a timeline starts: the graph frame of the transport origin, the seek position and an
/// optional loop. `playing` and `loop_enabled` are flags (any nonzero value is true).
pub struct ClockOrigin {
    pub graph_origin_frame: i64,
    pub start_beat: f64,
    pub playing: i32,
    pub loop_enabled: i32,
    pub loop_start_beat: f64,
    pub loop_end_beat: f64,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// The musical position of one graph frame.
pub struct ClockContext {
    pub revision: u64,
    pub epoch: u64,
    pub graph_frame: i64,
    pub content_graph_frame: i64,
    pub song_frame: i64,
    pub loop_iteration: i64,
    pub bar: i64,
    pub sample_rate: f64,
    pub beat: f64,
    pub bpm: f64,
    pub bar_start_beat: f64,
    pub bar_end_beat: f64,
    pub numerator: i32,
    pub denominator: i32,
    pub flags: i32,
    /// Graph-frame coordinate; `i64::MAX` when stopped.
    pub next_boundary_frame: i64,
    pub next_boundary_kinds: u32,
    pub loop_start_beat: f64,
    pub loop_end_beat: f64,
    /// Graph coordinate; `i64::MAX` if unreachable.
    pub next_quarter_beat_frame: i64,
    /// Musical-grid projection in graph coordinates, integrated through
    /// the immutable unwrapped map even when stopped or a short loop cannot reach an integer
    /// beat. This is a distance metric, not a transport prediction. `CLOCK_GRID_PROJECTION`
    /// marks an unavailable strict reachable target; adapters may then use this field for their
    /// documented grid convention.
    pub next_quarter_grid_frame: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// A context and the number of frames from its graph frame on that share it.
pub struct ClockSpan {
    pub context: ClockContext,
    /// Positive except at the inclusive map endpoint.
    pub frames: u32,
}

/// Why a context or span query produced nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockQueryError {
    /// A bad argument.
    Invalid,
    /// A frame outside the graph or song domain.
    OutsideDomain,
}

impl ClockQueryError {
    /// The status code of this error: `CLOCK_INVALID` or `CLOCK_OUTSIDE_DOMAIN`.
    pub const fn status(self) -> i32 {
        match self {
            Self::Invalid => CLOCK_INVALID,
            Self::OutsideDomain => CLOCK_OUTSIDE_DOMAIN,
        }
    }
}

impl std::fmt::Display for ClockQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "The clock query is out of range.",
            Self::OutsideDomain => "The frame lies outside the clock's domain.",
        })
    }
}

impl std::error::Error for ClockQueryError {}

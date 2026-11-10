//! The plain-data half of `Sources/TrackerEngine/include/TrackerClock.h`: structs and constants.
//!
//! Field order and types follow the header exactly; the assertions at the end pin every size and
//! offset to the values a C++ compiler produces for the header (the same on x86-64 and arm64).
//! Fields are named in snake case; the header's camel-case names are given where they differ.
//!
//! This file has no dependencies so that `unit_range` can use the types without the clock itself.

use std::mem::{align_of, offset_of, size_of};

// Query results. `ST_CLOCK_OK` is 1, not 0.
pub const ST_CLOCK_OK: i32 = 1;
pub const ST_CLOCK_INVALID: i32 = 0;
pub const ST_CLOCK_OUTSIDE_DOMAIN: i32 = -1;

// `STClockContext::flags`.
pub const ST_CLOCK_PLAYING: i32 = 1;
pub const ST_CLOCK_LOOPING: i32 = 2;
pub const ST_CLOCK_PREROLL: i32 = 4;
pub const ST_CLOCK_DOMAIN_END: i32 = 8;
pub const ST_CLOCK_RECORDING: i32 = 16;
pub const ST_CLOCK_GRID_PROJECTION: i32 = 32;

// `STClockContext::next_boundary_kinds`.
pub const ST_CLOCK_BOUNDARY_TEMPO: u32 = 1;
pub const ST_CLOCK_BOUNDARY_METER: u32 = 2;
pub const ST_CLOCK_BOUNDARY_BAR: u32 = 4;
pub const ST_CLOCK_BOUNDARY_LOOP: u32 = 8;
pub const ST_CLOCK_BOUNDARY_ORIGIN: u32 = 16;
pub const ST_CLOCK_BOUNDARY_DOMAIN: u32 = 32;

/// Most tempo events, and most meter events, in one map.
pub const ST_CLOCK_MAX_EVENTS: u32 = 1024;
/// Most frames in one span request.
pub const ST_CLOCK_MAX_SPAN: u32 = 65536;

/// `STClockTempoEvent`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockTempoEvent {
    pub beat: f64,
    pub bpm: f64,
}

/// `STClockMeterEvent`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockMeterEvent {
    pub beat: f64,
    pub numerator: i32,
    pub denominator: i32,
}

/// `STClockOrigin`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockOrigin {
    /// `graphOriginFrame`
    pub graph_origin_frame: i64,
    /// `startBeat`
    pub start_beat: f64,
    pub playing: i32,
    /// `loopEnabled`
    pub loop_enabled: i32,
    /// `loopStartBeat`
    pub loop_start_beat: f64,
    /// `loopEndBeat`
    pub loop_end_beat: f64,
    pub epoch: u64,
}

/// `STClockContext`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockContext {
    pub revision: u64,
    pub epoch: u64,
    /// `graphFrame`
    pub graph_frame: i64,
    /// `contentGraphFrame`
    pub content_graph_frame: i64,
    /// `songFrame`
    pub song_frame: i64,
    /// `loopIteration`
    pub loop_iteration: i64,
    pub bar: i64,
    /// `sampleRate`
    pub sample_rate: f64,
    pub beat: f64,
    pub bpm: f64,
    /// `barStartBeat`
    pub bar_start_beat: f64,
    /// `barEndBeat`
    pub bar_end_beat: f64,
    pub numerator: i32,
    pub denominator: i32,
    pub flags: i32,
    /// `nextBoundaryFrame`. Graph-frame coordinate; `i64::MAX` when stopped.
    pub next_boundary_frame: i64,
    /// `nextBoundaryKinds`
    pub next_boundary_kinds: u32,
    /// `loopStartBeat`
    pub loop_start_beat: f64,
    /// `loopEndBeat`
    pub loop_end_beat: f64,
    /// `nextQuarterBeatFrame`. Graph coordinate; `i64::MAX` if unreachable.
    pub next_quarter_beat_frame: i64,
    /// `nextQuarterGridFrame`. Musical-grid projection in graph coordinates, integrated through
    /// the immutable unwrapped map even when stopped or a short loop cannot reach an integer
    /// beat. This is a distance metric, not a transport prediction. `ST_CLOCK_GRID_PROJECTION`
    /// marks an unavailable strict reachable target; adapters may then use this field for their
    /// documented grid convention.
    pub next_quarter_grid_frame: i64,
}

/// `STClockSpan`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockSpan {
    pub context: STClockContext,
    /// Positive except at the inclusive map endpoint.
    pub frames: u32,
}

// Layout of every struct in TrackerClock.h, as printed by a C++ program using sizeof/offsetof.
const _: () = {
    assert!(size_of::<STClockTempoEvent>() == 16 && align_of::<STClockTempoEvent>() == 8);
    assert!(offset_of!(STClockTempoEvent, beat) == 0);
    assert!(offset_of!(STClockTempoEvent, bpm) == 8);

    assert!(size_of::<STClockMeterEvent>() == 16 && align_of::<STClockMeterEvent>() == 8);
    assert!(offset_of!(STClockMeterEvent, beat) == 0);
    assert!(offset_of!(STClockMeterEvent, numerator) == 8);
    assert!(offset_of!(STClockMeterEvent, denominator) == 12);

    assert!(size_of::<STClockOrigin>() == 48 && align_of::<STClockOrigin>() == 8);
    assert!(offset_of!(STClockOrigin, graph_origin_frame) == 0);
    assert!(offset_of!(STClockOrigin, start_beat) == 8);
    assert!(offset_of!(STClockOrigin, playing) == 16);
    assert!(offset_of!(STClockOrigin, loop_enabled) == 20);
    assert!(offset_of!(STClockOrigin, loop_start_beat) == 24);
    assert!(offset_of!(STClockOrigin, loop_end_beat) == 32);
    assert!(offset_of!(STClockOrigin, epoch) == 40);

    assert!(size_of::<STClockContext>() == 160 && align_of::<STClockContext>() == 8);
    assert!(offset_of!(STClockContext, revision) == 0);
    assert!(offset_of!(STClockContext, epoch) == 8);
    assert!(offset_of!(STClockContext, graph_frame) == 16);
    assert!(offset_of!(STClockContext, content_graph_frame) == 24);
    assert!(offset_of!(STClockContext, song_frame) == 32);
    assert!(offset_of!(STClockContext, loop_iteration) == 40);
    assert!(offset_of!(STClockContext, bar) == 48);
    assert!(offset_of!(STClockContext, sample_rate) == 56);
    assert!(offset_of!(STClockContext, beat) == 64);
    assert!(offset_of!(STClockContext, bpm) == 72);
    assert!(offset_of!(STClockContext, bar_start_beat) == 80);
    assert!(offset_of!(STClockContext, bar_end_beat) == 88);
    assert!(offset_of!(STClockContext, numerator) == 96);
    assert!(offset_of!(STClockContext, denominator) == 100);
    assert!(offset_of!(STClockContext, flags) == 104);
    assert!(offset_of!(STClockContext, next_boundary_frame) == 112);
    assert!(offset_of!(STClockContext, next_boundary_kinds) == 120);
    assert!(offset_of!(STClockContext, loop_start_beat) == 128);
    assert!(offset_of!(STClockContext, loop_end_beat) == 136);
    assert!(offset_of!(STClockContext, next_quarter_beat_frame) == 144);
    assert!(offset_of!(STClockContext, next_quarter_grid_frame) == 152);

    assert!(size_of::<STClockSpan>() == 168 && align_of::<STClockSpan>() == 8);
    assert!(offset_of!(STClockSpan, context) == 0);
    assert!(offset_of!(STClockSpan, frames) == 160);
};

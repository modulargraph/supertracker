//! The engine's use of the clock: C++ `EngineTiming` and `fixedClockContext` from
//! `TrackerEngine.cpp`.
//!
//! A project's optional stepped tempo/meter map is a [`crate::clock::STClockMap`] with an
//! always-playing [`crate::clock::STClockTimeline`] over it; without a map the legacy fixed-tempo
//! clock context is derived here.

use super::abi::*;
use super::util::{clamp, cmax, next_down};
use crate::clock::{STClockMap, STClockTimeline};
use std::sync::atomic::AtomicU32;

/// A validated tempo/meter map with its always-playing timeline (C++ `EngineTiming`).
///
/// Control owns map storage (`Arc`s held by projects and by `Control::timing_maps`); the render
/// thread's cache origin pins a map through `origin_pins` without touching the `Arc` count, so
/// releasing the pin on the render thread only changes an atomic and `collect()` owns
/// destruction. The clock objects are immutable; their queries allocate nothing and take no
/// locks, so the render thread may call them.
pub struct EngineTiming {
    pub origin_pins: AtomicU32,
    map: STClockMap,
    timeline: STClockTimeline,
    pub tempos: Vec<STClockTempoEvent>,
    pub meters: Vec<STClockMeterEvent>,
    pub revision: u64,
    /// Immutable map endpoint (frame of beat 1000000); not searched for every sample.
    pub maximum_frame: i64,
}

impl EngineTiming {
    /// Control thread. Copies and validates the complete event lists; the error is the clock's
    /// message.
    pub fn new(
        rate: f64,
        tempos: &[STClockTempoEvent],
        meters: &[STClockMeterEvent],
        revision: u64,
    ) -> Result<EngineTiming, String> {
        let map = STClockMap::new(rate, tempos, meters, revision).map_err(|problem| problem.message().to_string())?;
        let origin = STClockOrigin { playing: 1, ..STClockOrigin::default() };
        let timeline = STClockTimeline::new(&map, origin).map_err(|problem| problem.message().to_string())?;
        let mut timing = EngineTiming {
            origin_pins: AtomicU32::new(0),
            map,
            timeline,
            tempos: tempos.to_vec(),
            meters: meters.to_vec(),
            revision,
            maximum_frame: 0,
        };
        timing.maximum_frame = timing.frame(1000000.0);
        Ok(timing)
    }

    /// First sample not earlier than `beat` (clamped to the map domain).
    #[inline]
    pub fn frame(&self, beat: f64) -> i64 {
        self.map.frame_at_beat(clamp(beat, 0.0, 1000000.0)).unwrap_or(0)
    }

    /// Beat at a sample; 0 outside the map's sample domain.
    #[inline]
    pub fn beat(&self, frame: i64) -> f64 {
        self.map.beat_at_frame(frame).unwrap_or(0.0)
    }

    /// The timeline's context at song frame `frame` (`st_clock_context(timeline, frame, 0, &out)`);
    /// `out` is untouched unless the result is `ST_CLOCK_OK`.
    #[inline]
    pub fn context(&self, frame: i64, out: &mut STClockContext) -> i32 {
        match self.timeline.context(frame, 0) {
            Ok(context) => {
                *out = context;
                ST_CLOCK_OK
            }
            Err(problem) => problem.status(),
        }
    }

    pub fn matches(&self, other: &EngineTiming) -> bool {
        if self.revision != other.revision
            || self.tempos.len() != other.tempos.len()
            || self.meters.len() != other.meters.len()
        {
            return false;
        }
        for (a, b) in self.tempos.iter().zip(&other.tempos) {
            if a.beat != b.beat || a.bpm != b.bpm {
                return false;
            }
        }
        for (a, b) in self.meters.iter().zip(&other.meters) {
            if a.beat != b.beat || a.numerator != b.numerator || a.denominator != b.denominator {
                return false;
            }
        }
        true
    }
}

/// `fixedClockContext`: the clock context of the legacy (unmapped) transport.
///
/// Legacy seeks retain fractional frames and support BPM through 999. Derive boundaries relative
/// to that exact origin, including negative startup time.
pub fn fixed_clock_context(c: &mut STClockContext, frame: f64, rate: f64, bpm: f64, numerator: i32, denominator: i32) {
    *c = STClockContext::default();
    c.sample_rate = rate;
    c.song_frame = frame as i64;
    c.bpm = bpm;
    c.numerator = numerator;
    c.denominator = denominator;
    c.beat = frame * bpm / (60.0 * rate);
    let width = numerator as f64 * 4.0 / denominator as f64;
    let mut bar = (c.beat / width).floor() as i64;
    let offset = |beat: f64| -> i64 { (beat * (60.0 * rate) / bpm - frame).ceil() as i64 };
    for _ in 0..2 {
        if offset(bar as f64 * width) > 0 {
            bar = bar.wrapping_sub(1);
        } else if offset(bar.wrapping_add(1) as f64 * width) <= 0 {
            bar = bar.wrapping_add(1);
        } else {
            break;
        }
    }
    c.bar = bar.wrapping_add(1);
    c.bar_start_beat = bar as f64 * width;
    c.bar_end_beat = bar.wrapping_add(1) as f64 * width;
    c.beat = cmax(c.beat, c.bar_start_beat);
    if c.beat >= c.bar_end_beat {
        c.beat = next_down(c.bar_end_beat);
    }
    c.next_boundary_frame = c.song_frame.wrapping_add(offset(c.bar_end_beat));
    c.next_boundary_kinds = ST_CLOCK_BOUNDARY_BAR;
    let quarter = c.beat.floor();
    let mut next = offset(quarter);
    if next < 0 {
        next = offset(quarter + 1.0);
    }
    c.next_quarter_beat_frame = c.song_frame.wrapping_add(next);
    c.next_quarter_grid_frame = c.next_quarter_beat_frame;
}

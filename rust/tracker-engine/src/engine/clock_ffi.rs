//! The engine's use of the clock (`TrackerClock.h`): C++ `EngineTiming` and `fixedClockContext`
//! from `TrackerEngine.cpp`.
//!
//! The engine only talks to the clock through its C functions, exactly as the C++ engine did, so
//! the `st_clock_*` symbols are declared here and resolved at link time: by this crate's `clock`
//! module when that feature is built in, or by the C++ `TrackerClock.cpp`/`ClockTimingMap.cpp`
//! objects otherwise.

use super::abi::*;
use super::util::{clamp, cmax, next_down};
use std::ffi::{c_char, CStr};
use std::sync::atomic::AtomicU32;

#[repr(C)]
pub struct STClockMap {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct STClockTimeline {
    _opaque: [u8; 0],
}

extern "C" {
    fn st_clock_map_create(
        sample_rate: f64,
        tempos: *const STClockTempoEvent,
        tempo_count: u32,
        meters: *const STClockMeterEvent,
        meter_count: u32,
        revision: u64,
        error: *mut c_char,
        error_capacity: u32,
    ) -> *mut STClockMap;
    fn st_clock_map_destroy(map: *mut STClockMap);
    fn st_clock_frame_at_beat(map: *const STClockMap, beat: f64, frame: *mut i64) -> i32;
    fn st_clock_beat_at_frame(map: *const STClockMap, frame: i64, beat: *mut f64) -> i32;
    fn st_clock_timeline_create(
        map: *const STClockMap,
        origin: STClockOrigin,
        error: *mut c_char,
        error_capacity: u32,
    ) -> *mut STClockTimeline;
    fn st_clock_timeline_destroy(timeline: *mut STClockTimeline);
    fn st_clock_context(
        timeline: *const STClockTimeline,
        graph_frame: i64,
        upstream_latency_frames: i64,
        context: *mut STClockContext,
    ) -> i32;
}

/// A validated tempo/meter map with its always-playing timeline (C++ `EngineTiming`).
///
/// Control owns map storage (`Arc`s held by projects and by `Control::timing_maps`); the render
/// thread's cache origin pins a map through `origin_pins` without touching the `Arc` count, so
/// releasing the pin on the render thread only changes an atomic and `collect()` owns destruction.
pub struct EngineTiming {
    pub origin_pins: AtomicU32,
    map: *mut STClockMap,
    timeline: *mut STClockTimeline,
    pub tempos: Vec<STClockTempoEvent>,
    pub meters: Vec<STClockMeterEvent>,
    pub revision: u64,
    /// Immutable map endpoint (frame of beat 1000000); not searched for every sample.
    pub maximum_frame: i64,
}

// SAFETY: the clock map and timeline are immutable after creation and their query functions
// "allocate nothing, take no locks and never mutate state" (TrackerClock.h), so sharing them
// between the control and render threads is sound. They are destroyed only in Drop, which runs
// on the control thread once no project and no cache-origin pin refers to this map.
unsafe impl Send for EngineTiming {}
unsafe impl Sync for EngineTiming {}

fn error_text(buffer: &[c_char; 256]) -> String {
    // SAFETY: the buffer is NUL-terminated: it starts zeroed and the clock writes at most
    // `capacity - 1` characters plus a terminator.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().into_owned()
}

impl EngineTiming {
    /// Control thread. `tempos`/`meters` are the caller's arrays (validated by the clock).
    ///
    /// # Safety
    /// `tempos` must point to `tempo_count` events and `meters` to `meter_count` events (either
    /// may be null when its count is zero).
    pub unsafe fn new(
        rate: f64,
        tempos: *const STClockTempoEvent,
        tempo_count: u32,
        meters: *const STClockMeterEvent,
        meter_count: u32,
        revision: u64,
    ) -> Result<EngineTiming, String> {
        let mut error = [0 as c_char; 256];
        // SAFETY: the pointers and counts are forwarded unchanged under this function's contract;
        // the error buffer is writable for the stated capacity.
        let map = unsafe {
            st_clock_map_create(rate, tempos, tempo_count, meters, meter_count, revision, error.as_mut_ptr(), 256)
        };
        if map.is_null() {
            return Err(error_text(&error));
        }
        let origin = STClockOrigin { playing: 1, ..STClockOrigin::default() };
        // SAFETY: `map` is the valid map just created.
        let timeline = unsafe { st_clock_timeline_create(map, origin, error.as_mut_ptr(), 256) };
        if timeline.is_null() {
            // SAFETY: `map` is valid and not referenced by any timeline.
            unsafe { st_clock_map_destroy(map) };
            return Err(error_text(&error));
        }
        // SAFETY: map creation succeeded, so the clock has read (and validated) exactly these
        // `tempo_count`/`meter_count` elements; both counts are at least 1.
        let (tempo_list, meter_list) = unsafe {
            (
                std::slice::from_raw_parts(tempos, tempo_count as usize).to_vec(),
                std::slice::from_raw_parts(meters, meter_count as usize).to_vec(),
            )
        };
        let mut timing = EngineTiming {
            origin_pins: AtomicU32::new(0),
            map,
            timeline,
            tempos: tempo_list,
            meters: meter_list,
            revision,
            maximum_frame: 0,
        };
        timing.maximum_frame = timing.frame(1000000.0);
        Ok(timing)
    }

    /// First sample not earlier than `beat` (clamped to the map domain).
    #[inline]
    pub fn frame(&self, beat: f64) -> i64 {
        let mut result = 0i64;
        // SAFETY: `map` is valid for the lifetime of `self`; `result` is a valid out pointer.
        unsafe { st_clock_frame_at_beat(self.map, clamp(beat, 0.0, 1000000.0), &mut result) };
        result
    }

    #[inline]
    pub fn beat(&self, frame: i64) -> f64 {
        let mut result = 0f64;
        // SAFETY: as in `frame`.
        unsafe { st_clock_beat_at_frame(self.map, frame, &mut result) };
        result
    }

    /// `st_clock_context(timeline, frame, 0, out)`; `out` is untouched unless ST_CLOCK_OK.
    #[inline]
    pub fn context(&self, frame: i64, out: &mut STClockContext) -> i32 {
        // SAFETY: `timeline` is valid for the lifetime of `self`; `out` is a valid out pointer.
        unsafe { st_clock_context(self.timeline, frame, 0, out) }
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

impl Drop for EngineTiming {
    fn drop(&mut self) {
        // SAFETY: both handles were created in `new`, are owned by this value alone and are
        // destroyed exactly once, the timeline (which refers to the map) first.
        unsafe {
            st_clock_timeline_destroy(self.timeline);
            st_clock_map_destroy(self.map);
        }
    }
}

/// `fixedClockContext`: the clock context of the legacy (unmapped) transport.
///
/// Legacy seeks retain fractional frames and support BPM through 999. Derive boundaries relative
/// to that exact origin, including negative startup time.
pub fn fixed_clock_context(c: &mut STClockContext, frame: f64, rate: f64, bpm: f64, numerator: i32, denominator: i32) {
    *c = STClockContext::default();
    c.sampleRate = rate;
    c.songFrame = frame as i64;
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
    c.barStartBeat = bar as f64 * width;
    c.barEndBeat = bar.wrapping_add(1) as f64 * width;
    c.beat = cmax(c.beat, c.barStartBeat);
    if c.beat >= c.barEndBeat {
        c.beat = next_down(c.barEndBeat);
    }
    c.nextBoundaryFrame = c.songFrame.wrapping_add(offset(c.barEndBeat));
    c.nextBoundaryKinds = ST_CLOCK_BOUNDARY_BAR;
    let quarter = c.beat.floor();
    let mut next = offset(quarter);
    if next < 0 {
        next = offset(quarter + 1.0);
    }
    c.nextQuarterBeatFrame = c.songFrame.wrapping_add(next);
    c.nextQuarterGridFrame = c.nextQuarterBeatFrame;
}

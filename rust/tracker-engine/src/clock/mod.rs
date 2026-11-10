//! The sample-accurate musical clock: the C API of `Sources/TrackerEngine/include/TrackerClock.h`.
//!
//! Replaces `Sources/TrackerEngine/TrackerClock.cpp` (and, in [`timing_map`], `ClockTimingMap.cpp`).
//! The translation is deliberately literal: same numeric types, same order of floating-point
//! operations, same fused multiply-adds, so every frame and beat equals the C++ engine's.
//!
//! Immutable timing foundation; optionally adopted by TrackerEngine playback. Creation and
//! destruction are control-thread operations. Queries allocate nothing, take no locks and never
//! mutate state. Destroy only after readers are quiescent.
//!
//! Rust callers can use [`STClockMap`] and [`STClockTimeline`] directly through their safe
//! methods; the `st_clock_*` functions are thin wrappers that add the C null-pointer rules.
//!
//! Differences from the C++ that cannot be observed through valid use:
//! * Running out of memory while creating a map or timeline aborts instead of reporting an error.
//! * Where the C++ dereferenced an empty `std::optional` (undefined behaviour, reachable only if
//!   inverse rounding puts the last sample's beat above 1000000), `st_clock_context` returns
//!   `ST_CLOCK_OUTSIDE_DOMAIN`.

#![deny(unsafe_op_in_unsafe_fn)]

mod abi;
mod cxx_math;
mod long_double;
pub mod timing_map;
#[cfg(any(
    all(
        any(target_arch = "x86", target_arch = "x86_64"),
        not(target_env = "msvc")
    ),
    test
))]
mod x87;

#[cfg(test)]
mod tests;

use std::ffi::c_char;
use std::fmt;
use std::sync::Arc;

pub use abi::*;
use timing_map::{MeterEvent, TempoEvent, TimingMap, TimingMapError};

/// Graph frames and graph origins are within +/-2^52.
const GRAPH_LIMIT: i64 = 1 << 52;
/// Upstream latency is 0..2^30 frames.
const LATENCY_LIMIT: i64 = 1 << 30;

/// Why a map or timeline could not be created. [`ClockError::message`] is the text copied to the
/// caller of the C API; the C++ tests compare parts of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    /// The sample rate is not finite or outside 8000..384000.
    SampleRate,
    /// A missing event list, or not 1..1024 events of each kind.
    EventCount,
    /// The events do not form a valid timing map.
    Map(TimingMapError),
    /// Two different tempo/meter positions round up to the same sample.
    BoundariesCollapse,
    /// No map was given for a timeline.
    MapRequired,
    /// The graph origin or the seek beat is out of range.
    Origin,
    /// The loop beats are out of range, reversed, or not after the seek position.
    Loop,
    /// The loop or seek positions round up to the same sample.
    LoopCollapse,
}

impl ClockError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::SampleRate => "Sample rate must be finite and within 8000..384000 Hz.",
            Self::EventCount => "Provide 1..1024 tempo and meter events, beginning at quarter beat zero.",
            Self::Map(error) => error.message(),
            Self::BoundariesCollapse => {
                "Distinct tempo/meter positions collapse onto one audio sample; move them apart or use the same quarter-beat position."
            }
            Self::MapRequired => "A timing map is required.",
            Self::Origin => "Graph origin must be within +/-2^52 frames and seek beat within 0..1000000.",
            Self::Loop => "Loop requires 0<=start<end<=1000000 and a seek position before its end.",
            Self::LoopCollapse => "Loop/seek positions collapse onto the same audio sample.",
        }
    }
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ClockError {}

impl From<TimingMapError> for ClockError {
    fn from(error: TimingMapError) -> Self {
        Self::Map(error)
    }
}

/// Why a context or span query produced nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockQueryError {
    /// A bad argument: `ST_CLOCK_INVALID`.
    Invalid,
    /// A frame outside the graph or song domain: `ST_CLOCK_OUTSIDE_DOMAIN`.
    OutsideDomain,
}

impl ClockQueryError {
    /// The C result code.
    pub const fn status(self) -> i32 {
        match self {
            Self::Invalid => ST_CLOCK_INVALID,
            Self::OutsideDomain => ST_CLOCK_OUTSIDE_DOMAIN,
        }
    }
}

/// A tempo and/or meter change on its first audio sample.
struct Boundary {
    beat: f64,
    frame: i64,
    kinds: u32,
}

/// A timing map compiled for one sample rate. Shared, immutable.
struct ClockData {
    rate: f64,
    revision: u64,
    map: TimingMap,
    end_frame: i64,
    boundaries: Vec<Boundary>,
}

fn valid_rate(rate: f64) -> bool {
    rate.is_finite() && (8000.0..=384000.0).contains(&rate)
}

fn valid_frame(frame: i64) -> bool {
    (-GRAPH_LIMIT..=GRAPH_LIMIT).contains(&frame)
}

fn valid_beat(beat: f64) -> bool {
    beat.is_finite() && (0.0..=TimingMap::MAX_BEAT).contains(&beat)
}

impl ClockData {
    fn new(
        rate: f64,
        tempos: &[TempoEvent],
        meters: &[MeterEvent],
        revision: u64,
    ) -> Result<Self, ClockError> {
        let map = TimingMap::new(tempos, meters)?;
        let end_frame = (map.max_seconds() * rate).ceil() as i64;
        let mut data = Self {
            rate,
            revision,
            map,
            end_frame,
            boundaries: Vec::with_capacity(tempos.len() + meters.len()),
        };
        // Merge both event lists by position. Same-position tempo+meter events are atomic.
        let (mut t, mut m) = (0, 0);
        while t < tempos.len() || m < meters.len() {
            let mut beat = tempos.get(t).map_or(f64::INFINITY, |event| event.beat);
            if let Some(event) = meters.get(m) {
                beat = cxx_math::min(beat, event.beat);
            }
            let mut kinds = 0;
            if tempos.get(t).is_some_and(|event| event.beat == beat) {
                kinds |= ST_CLOCK_BOUNDARY_TEMPO;
                t += 1;
            }
            if meters.get(m).is_some_and(|event| event.beat == beat) {
                kinds |= ST_CLOCK_BOUNDARY_METER;
                m += 1;
            }
            let frame = data.raw_frame_at_beat(beat);
            if data
                .boundaries
                .last()
                .is_some_and(|last| frame <= last.frame)
            {
                return Err(ClockError::BoundariesCollapse);
            }
            data.boundaries.push(Boundary { beat, frame, kinds });
        }
        Ok(data)
    }

    /// Tempo before the first sample: negative song time extrapolates the initial tempo.
    fn initial_bpm(&self) -> f64 {
        // A compiled map always has a first tempo.
        self.map
            .tempos()
            .first()
            .map_or(TimingMap::MIN_BPM, |segment| segment.bpm)
    }

    fn initial_frame_at_beat(&self, beat: f64) -> i64 {
        let mut frame = (beat * (60.0 * self.rate) / self.initial_bpm()).ceil() as i64;
        // Far preroll can approach 2^53 elapsed frames even though each graph
        // coordinate is bounded to 2^52. Its floating ULP can span a few frames.
        for _ in 0..8 {
            if self.beat_at_frame(frame - 1) >= beat {
                frame -= 1;
            } else if self.beat_at_frame(frame) < beat {
                frame += 1;
            } else {
                break;
            }
        }
        frame
    }

    fn raw_frame_at_beat(&self, beat: f64) -> i64 {
        // Every caller passes a beat inside the closed domain (the C++ dereferences the optional
        // unchecked); zero is only a defined result for the impossible case.
        self.map
            .seconds_at_beat(beat)
            .map_or(0, |seconds| (seconds * self.rate).ceil() as i64)
    }

    fn frame_at_beat(&self, beat: f64) -> i64 {
        let mut frame = self.raw_frame_at_beat(beat);
        // At this bounded domain, floating inverse error is much smaller than one
        // sample. Exact neighboring comparisons make sampled round trips stable
        // without an epsilon that could swallow an intentionally late event.
        for _ in 0..2 {
            if frame > 0 && self.beat_at_frame(frame - 1) >= beat {
                frame -= 1;
            } else if frame < self.end_frame && self.beat_at_frame(frame) < beat {
                frame += 1;
            } else {
                break;
            }
        }
        frame
    }

    fn beat_at_frame(&self, frame: i64) -> f64 {
        if frame < 0 {
            return frame as f64 * self.initial_bpm() / (60.0 * self.rate);
        }
        if frame == self.end_frame {
            return TimingMap::MAX_BEAT;
        }
        // Invert directly in sample coordinates. Rounding frame/rate first can
        // place an exactly aligned beat just below itself (64000/48000 after a
        // 60->180 BPM change), incorrectly delaying it in frameAtBeat's strict
        // neighboring comparison. FMA also retains fractional prefix samples.
        let segments = self.map.tempos();
        // `std::upper_bound` with the C++ comparator, kept in its own terms.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        let next_tempo = segments
            .partition_point(|segment| !((frame as f64) < (segment.seconds * self.rate).ceil()));
        // The first segment starts at sample zero, so one always precedes a non-negative frame.
        let Some(segment) = segments.get(next_tempo.saturating_sub(1)) else {
            return 0.0;
        };
        let delta_frames = (-segment.seconds).mul_add(self.rate, frame as f64);
        let mut beat = delta_frames.mul_add(segment.bpm / (60.0 * self.rate), segment.beat);
        // Compiled sample boundaries are authoritative. These one-sided corrections
        // resolve only floating-point inverse rounding, never epsilon-snap input.
        let next = self
            .boundaries
            .partition_point(|boundary| !(frame < boundary.frame));
        if let Some(previous) = next
            .checked_sub(1)
            .and_then(|index| self.boundaries.get(index))
        {
            beat = cxx_math::max(beat, previous.beat);
        }
        if let Some(following) = self.boundaries.get(next) {
            if beat >= following.beat {
                beat = cxx_math::next_below(following.beat);
            }
        }
        beat
    }
}

/// `STClockMap`: an immutable timing map compiled for one sample rate.
///
/// 1..1024 ascending unique events of each kind, both beginning at beat 0. Beats 0..1000000,
/// BPM 20..400, meter 1..16 with denominator 2/4/8/16, sampleRate 8000..384000. Same-position
/// tempo+meter events are atomic. Distinct positions collapsing onto one sample are rejected.
///
/// Conversions are absolute ceil-to-sample: the first sample not earlier than the mapped event,
/// with no epsilon snapping. The continuous seconds law follows [`TimingMap`]. Continuous beats
/// generally quantize: the inverse at the returned sample can be less than one sample later.
/// Sample-domain endpoints are inclusive; their beat is exactly 1000000. Negative frames are
/// valid initial-tempo preroll.
pub struct STClockMap {
    data: Arc<ClockData>,
}

impl STClockMap {
    pub fn new(
        sample_rate: f64,
        tempos: &[STClockTempoEvent],
        meters: &[STClockMeterEvent],
        revision: u64,
    ) -> Result<Self, ClockError> {
        if !valid_rate(sample_rate) {
            return Err(ClockError::SampleRate);
        }
        let allowed = 1..=ST_CLOCK_MAX_EVENTS as usize;
        if !allowed.contains(&tempos.len()) || !allowed.contains(&meters.len()) {
            return Err(ClockError::EventCount);
        }
        let tempos: Vec<TempoEvent> = tempos
            .iter()
            .map(|event| TempoEvent {
                beat: event.beat,
                bpm: event.bpm,
            })
            .collect();
        let meters: Vec<MeterEvent> = meters
            .iter()
            .map(|event| MeterEvent {
                beat: event.beat,
                numerator: event.numerator,
                denominator: event.denominator,
            })
            .collect();
        Ok(Self {
            data: Arc::new(ClockData::new(sample_rate, &tempos, &meters, revision)?),
        })
    }

    /// First sample not earlier than `beat`; `None` outside 0..1000000.
    pub fn frame_at_beat(&self, beat: f64) -> Option<i64> {
        valid_beat(beat).then(|| self.data.frame_at_beat(beat))
    }

    /// Beat at a sample; `None` outside -2^52..=the map's last sample.
    pub fn beat_at_frame(&self, frame: i64) -> Option<f64> {
        (valid_frame(frame) && frame <= self.data.end_frame).then(|| self.data.beat_at_frame(frame))
    }

    pub fn seconds_at_beat(&self, beat: f64) -> Option<f64> {
        self.data.map.seconds_at_beat(beat)
    }

    pub fn beat_at_seconds(&self, seconds: f64) -> Option<f64> {
        self.data.map.beat_at_seconds(seconds)
    }
}

/// `STClockTimeline`: a map plus a transport origin.
///
/// Timeline owns a map reference; the caller may destroy its `STClockMap` handle. Graph origin is
/// within +/-2^52 frames. A seek quantizes startBeat once. Loops require
/// 0<=start<end<=1000000 and startBeat<loopEndBeat; an intro before loopStart is allowed.
/// Quantized loops repeat [startFrame,endFrame) exactly. Queries before graphOrigin do not wrap
/// backwards and are marked preroll. A stopped timeline holds its quantized seek position and
/// never advances.
pub struct STClockTimeline {
    data: Arc<ClockData>,
    /// `playing` and `loop_enabled` are normalized to 0 or 1.
    origin: STClockOrigin,
    start_frame: i64,
    /// Zero unless looping; otherwise `loop_start < loop_end`.
    loop_start: i64,
    loop_end: i64,
}

/// The nearest boundary ahead of the current sample, with all kinds that fall on it.
struct NextBoundary {
    delta: i64,
    kinds: u32,
}

impl NextBoundary {
    fn consider(&mut self, distance: i64, kinds: u32) {
        if distance < 0 {
            return;
        }
        if distance < self.delta {
            self.delta = distance;
            self.kinds = kinds;
        } else if distance == self.delta {
            self.kinds |= kinds;
        }
    }
}

impl STClockTimeline {
    pub fn new(map: &STClockMap, mut origin: STClockOrigin) -> Result<Self, ClockError> {
        if !valid_frame(origin.graph_origin_frame) || !valid_beat(origin.start_beat) {
            return Err(ClockError::Origin);
        }
        let data = &map.data;
        let start_frame = data.frame_at_beat(origin.start_beat);
        let (mut loop_start, mut loop_end) = (0, 0);
        if origin.loop_enabled != 0 {
            if !valid_beat(origin.loop_start_beat)
                || !valid_beat(origin.loop_end_beat)
                || origin.loop_start_beat >= origin.loop_end_beat
                || origin.start_beat >= origin.loop_end_beat
            {
                return Err(ClockError::Loop);
            }
            loop_start = data.frame_at_beat(origin.loop_start_beat);
            loop_end = data.frame_at_beat(origin.loop_end_beat);
            if loop_start >= loop_end || start_frame >= loop_end {
                return Err(ClockError::LoopCollapse);
            }
        }
        origin.playing = (origin.playing != 0) as i32;
        origin.loop_enabled = (origin.loop_enabled != 0) as i32;
        Ok(Self {
            data: Arc::clone(data),
            origin,
            start_frame,
            loop_start,
            loop_end,
        })
    }

    /// Musical context at one graph frame.
    ///
    /// `graph_frame` is continuous graph time, within +/-2^52. `upstream_latency_frames` is
    /// 0..2^30 and is subtracted before musical lookup; own output latency is not subtracted.
    /// Negative song time extrapolates initial tempo/meter and uses bar 0, then negative bar
    /// numbers. All next-boundary flags at one frame are combined. Queries do not render audio,
    /// consume events, apply pending commands, or access live status.
    pub fn context(
        &self,
        graph_frame: i64,
        upstream_latency_frames: i64,
    ) -> Result<STClockContext, ClockQueryError> {
        let latency = upstream_latency_frames;
        if !(0..=LATENCY_LIMIT).contains(&latency) {
            return Err(ClockQueryError::Invalid);
        }
        if !valid_frame(graph_frame) {
            return Err(ClockQueryError::OutsideDomain);
        }
        let data = &*self.data;
        let origin = &self.origin;
        let playing = origin.playing != 0;
        let looping = origin.loop_enabled != 0;
        let mut c = STClockContext {
            revision: data.revision,
            epoch: origin.epoch,
            graph_frame,
            content_graph_frame: graph_frame - latency,
            sample_rate: data.rate,
            ..STClockContext::default()
        };
        // All frames here are bounded: graph coordinates by 2^52, latency by 2^30 and song
        // frames by the map's last sample (about 2^40), so sums of a few of them fit easily.
        let elapsed = if playing {
            c.content_graph_frame - origin.graph_origin_frame
        } else {
            0
        };
        c.song_frame = self.start_frame + elapsed;
        if playing {
            c.flags |= ST_CLOCK_PLAYING;
        }
        if looping {
            c.flags |= ST_CLOCK_LOOPING;
        }
        if playing && elapsed < 0 {
            c.flags |= ST_CLOCK_PREROLL;
        }
        if looping && elapsed >= 0 && c.song_frame >= self.loop_end {
            let width = self.loop_end - self.loop_start;
            let distance = c.song_frame - self.loop_end;
            c.loop_iteration = 1 + distance / width;
            c.song_frame = self.loop_start + distance % width;
        }
        if c.song_frame > data.end_frame {
            return Err(ClockQueryError::OutsideDomain);
        }
        c.beat = data.beat_at_frame(c.song_frame);
        if c.song_frame < 0 {
            // A compiled map always has a first meter.
            let Some(meter) = data.map.meters().first() else {
                return Err(ClockQueryError::OutsideDomain);
            };
            c.bpm = data.initial_bpm();
            c.numerator = meter.numerator;
            c.denominator = meter.denominator;
            let mut offset = (c.beat / meter.quarters_per_bar).floor() as i64;
            // Negative preroll has no compiled meter segment. Use absolute sample
            // boundaries here too: an inverse a fraction below an exact bar line
            // must not select a completed bar and then skip its end entirely.
            for _ in 0..2 {
                let start = data.initial_frame_at_beat(offset as f64 * meter.quarters_per_bar);
                let end = data.initial_frame_at_beat((offset + 1) as f64 * meter.quarters_per_bar);
                if c.song_frame < start {
                    offset -= 1;
                } else if c.song_frame >= end {
                    offset += 1;
                } else {
                    break;
                }
            }
            c.bar = offset + 1;
            c.bar_start_beat = offset as f64 * meter.quarters_per_bar;
            c.bar_end_beat = (offset + 1) as f64 * meter.quarters_per_bar;
            c.beat = cxx_math::max(c.beat, c.bar_start_beat);
            if c.beat >= c.bar_end_beat {
                c.beat = cxx_math::next_below(c.bar_end_beat);
            }
        } else {
            // See the module documentation: empty only where the C++ behaviour is undefined.
            let (Some(bpm), Some(bar)) =
                (data.map.tempo_at_beat(c.beat), data.map.bar_at_beat(c.beat))
            else {
                return Err(ClockQueryError::OutsideDomain);
            };
            c.bpm = bpm;
            c.bar = bar.bar as i64;
            c.bar_start_beat = bar.bar_start;
            c.bar_end_beat = bar.bar_end;
            c.numerator = bar.numerator;
            c.denominator = bar.denominator;
        }
        c.next_boundary_frame = i64::MAX;
        c.next_quarter_beat_frame = i64::MAX;
        c.next_quarter_grid_frame = i64::MAX;
        c.loop_start_beat = if looping { origin.loop_start_beat } else { 0.0 };
        c.loop_end_beat = if looping { origin.loop_end_beat } else { 0.0 };
        if c.song_frame == data.end_frame {
            c.flags |= ST_CLOCK_DOMAIN_END;
        }
        let mut quarter = c.beat.floor();
        let frame_for_quarter = |beat: f64| {
            if beat < 0.0 {
                data.initial_frame_at_beat(beat)
            } else {
                data.frame_at_beat(beat)
            }
        };
        let mut quarter_frame = frame_for_quarter(quarter);
        if quarter_frame < c.song_frame {
            quarter += 1.0;
            if quarter <= TimingMap::MAX_BEAT {
                quarter_frame = frame_for_quarter(quarter);
            }
        }
        if quarter <= TimingMap::MAX_BEAT && quarter_frame >= c.song_frame {
            c.next_quarter_grid_frame = graph_frame + quarter_frame - c.song_frame;
        }
        if playing {
            let mut next = NextBoundary {
                delta: i64::MAX,
                kinds: 0,
            };
            let following = data
                .boundaries
                .partition_point(|boundary| !(c.song_frame < boundary.frame));
            if let Some(boundary) = data.boundaries.get(following) {
                next.consider(boundary.frame - c.song_frame, boundary.kinds);
            }
            if c.bar_end_beat <= TimingMap::MAX_BEAT {
                let bar_end = if c.bar_end_beat < 0.0 {
                    data.initial_frame_at_beat(c.bar_end_beat)
                } else {
                    data.frame_at_beat(c.bar_end_beat)
                };
                if bar_end > c.song_frame {
                    next.consider(bar_end - c.song_frame, ST_CLOCK_BOUNDARY_BAR);
                }
            }
            next.consider(data.end_frame - c.song_frame, ST_CLOCK_BOUNDARY_DOMAIN);
            if looping {
                next.consider(self.loop_end - c.song_frame, ST_CLOCK_BOUNDARY_LOOP);
            }
            if elapsed < 0 {
                next.consider(-elapsed, ST_CLOCK_BOUNDARY_ORIGIN);
            }
            if next.delta != i64::MAX {
                c.next_boundary_frame = graph_frame + next.delta;
                c.next_boundary_kinds = next.kinds;
            }
            if looping && elapsed >= 0 && quarter_frame >= self.loop_end {
                let first = origin.loop_start_beat.ceil();
                if first < origin.loop_end_beat {
                    let first_frame = frame_for_quarter(first);
                    if first_frame < self.loop_end {
                        c.next_quarter_beat_frame = graph_frame + self.loop_end - c.song_frame
                            + first_frame
                            - self.loop_start;
                    }
                }
            } else if quarter <= TimingMap::MAX_BEAT && quarter_frame >= c.song_frame {
                c.next_quarter_beat_frame = graph_frame + quarter_frame - c.song_frame;
            }
        }
        if c.next_quarter_beat_frame == i64::MAX {
            c.flags |= ST_CLOCK_GRID_PROJECTION;
        }
        Ok(c)
    }

    /// Context at `graph_frame` and how many of `requested_frames` (1..65536) share it.
    ///
    /// A span ends at tempo/meter/bar/loop/origin/domain boundaries and at the graph limit.
    /// The host owns cached-range retention and publishing/replacing timeline objects at
    /// quiescent render boundaries.
    pub fn span(
        &self,
        graph_frame: i64,
        requested_frames: u32,
        upstream_latency_frames: i64,
    ) -> Result<STClockSpan, ClockQueryError> {
        if !(1..=ST_CLOCK_MAX_SPAN).contains(&requested_frames) {
            return Err(ClockQueryError::Invalid);
        }
        let context = self.context(graph_frame, upstream_latency_frames)?;
        let mut count = i64::from(requested_frames).min(GRAPH_LIMIT - graph_frame + 1);
        if context.next_boundary_frame != i64::MAX {
            count = count.min(context.next_boundary_frame - graph_frame);
        }
        // `count` is at most `requested_frames`.
        Ok(STClockSpan {
            context,
            frames: count.max(0) as u32,
        })
    }
}

/// Copies `message` to caller storage of `capacity` bytes, truncated and always NUL terminated.
///
/// # Safety
/// `out` is null or valid for writes of `capacity` bytes.
unsafe fn error_text(out: *mut c_char, capacity: u32, message: &str) {
    if out.is_null() || capacity == 0 {
        return;
    }
    let count = message.len().min(capacity as usize - 1);
    // SAFETY: `count + 1 <= capacity` bytes are written to storage the caller vouches for, and a
    // Rust string never overlaps caller-owned C storage that is valid for writes.
    unsafe {
        std::ptr::copy_nonoverlapping(message.as_ptr(), out.cast::<u8>(), count);
        out.add(count).write(0);
    }
}

/// Reports `result` the C way: the object on the heap, or null and the message in `error`.
///
/// # Safety
/// As for [`error_text`].
unsafe fn into_handle<T>(
    result: Result<T, ClockError>,
    error: *mut c_char,
    capacity: u32,
) -> *mut T {
    match result {
        Ok(value) => Box::into_raw(Box::new(value)),
        Err(failure) => {
            // SAFETY: forwarded from the caller.
            unsafe { error_text(error, capacity, failure.message()) };
            std::ptr::null_mut()
        }
    }
}

/// Error text is copied to caller storage (optional); no partial object is returned on error.
///
/// # Safety
/// `tempos` and `meters` are null or point to `tempo_count` / `meter_count` events; `error` is
/// null or valid for writes of `error_capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn st_clock_map_create(
    sample_rate: f64,
    tempos: *const STClockTempoEvent,
    tempo_count: u32,
    meters: *const STClockMeterEvent,
    meter_count: u32,
    revision: u64,
    error: *mut c_char,
    error_capacity: u32,
) -> *mut STClockMap {
    // SAFETY: the caller's contract for `error`.
    unsafe { error_text(error, error_capacity, "") };
    let allowed = 1..=ST_CLOCK_MAX_EVENTS;
    // Same order as the C++: the sample rate is checked before the event lists are looked at.
    let result = if !valid_rate(sample_rate) {
        Err(ClockError::SampleRate)
    } else if tempos.is_null()
        || meters.is_null()
        || !allowed.contains(&tempo_count)
        || !allowed.contains(&meter_count)
    {
        Err(ClockError::EventCount)
    } else {
        // SAFETY: both pointers are non-null and, by the caller's contract, address `tempo_count`
        // and `meter_count` (1..1024) initialized events that nothing mutates during this call.
        let (tempos, meters) = unsafe {
            (
                std::slice::from_raw_parts(tempos, tempo_count as usize),
                std::slice::from_raw_parts(meters, meter_count as usize),
            )
        };
        STClockMap::new(sample_rate, tempos, meters, revision)
    };
    // SAFETY: the caller's contract for `error`.
    unsafe { into_handle(result, error, error_capacity) }
}

/// # Safety
/// `map` is null or came from [`st_clock_map_create`] and is not used again. Timelines created
/// from it stay valid.
#[no_mangle]
pub unsafe extern "C" fn st_clock_map_destroy(map: *mut STClockMap) {
    if !map.is_null() {
        // SAFETY: the pointer came from `Box::into_raw` in `st_clock_map_create` and ownership
        // returns here exactly once.
        drop(unsafe { Box::from_raw(map) });
    }
}

/// On error, output values are left untouched.
///
/// # Safety
/// `map` is null or a live map; `frame` is null or valid for a write.
#[no_mangle]
pub unsafe extern "C" fn st_clock_frame_at_beat(
    map: *const STClockMap,
    beat: f64,
    frame: *mut i64,
) -> i32 {
    // SAFETY: a non-null `map` is a live, immutable map by the caller's contract.
    let Some(map) = (unsafe { map.as_ref() }) else {
        return ST_CLOCK_INVALID;
    };
    if frame.is_null() {
        return ST_CLOCK_INVALID;
    }
    let Some(value) = map.frame_at_beat(beat) else {
        return ST_CLOCK_OUTSIDE_DOMAIN;
    };
    // SAFETY: `frame` is non-null and writable by the caller's contract.
    unsafe { frame.write(value) };
    ST_CLOCK_OK
}

/// # Safety
/// `map` is null or a live map; `beat` is null or valid for a write.
#[no_mangle]
pub unsafe extern "C" fn st_clock_beat_at_frame(
    map: *const STClockMap,
    frame: i64,
    beat: *mut f64,
) -> i32 {
    // SAFETY: a non-null `map` is a live, immutable map by the caller's contract.
    let Some(map) = (unsafe { map.as_ref() }) else {
        return ST_CLOCK_INVALID;
    };
    if beat.is_null() {
        return ST_CLOCK_INVALID;
    }
    let Some(value) = map.beat_at_frame(frame) else {
        return ST_CLOCK_OUTSIDE_DOMAIN;
    };
    // SAFETY: `beat` is non-null and writable by the caller's contract.
    unsafe { beat.write(value) };
    ST_CLOCK_OK
}

/// # Safety
/// `map` is null or a live map; `seconds` is null or valid for a write.
#[no_mangle]
pub unsafe extern "C" fn st_clock_seconds_at_beat(
    map: *const STClockMap,
    beat: f64,
    seconds: *mut f64,
) -> i32 {
    // SAFETY: a non-null `map` is a live, immutable map by the caller's contract.
    let Some(map) = (unsafe { map.as_ref() }) else {
        return ST_CLOCK_INVALID;
    };
    if seconds.is_null() {
        return ST_CLOCK_INVALID;
    }
    let Some(value) = map.seconds_at_beat(beat) else {
        return ST_CLOCK_OUTSIDE_DOMAIN;
    };
    // SAFETY: `seconds` is non-null and writable by the caller's contract.
    unsafe { seconds.write(value) };
    ST_CLOCK_OK
}

/// # Safety
/// `map` is null or a live map; `beat` is null or valid for a write.
#[no_mangle]
pub unsafe extern "C" fn st_clock_beat_at_seconds(
    map: *const STClockMap,
    seconds: f64,
    beat: *mut f64,
) -> i32 {
    // SAFETY: a non-null `map` is a live, immutable map by the caller's contract.
    let Some(map) = (unsafe { map.as_ref() }) else {
        return ST_CLOCK_INVALID;
    };
    if beat.is_null() {
        return ST_CLOCK_INVALID;
    }
    let Some(value) = map.beat_at_seconds(seconds) else {
        return ST_CLOCK_OUTSIDE_DOMAIN;
    };
    // SAFETY: `beat` is non-null and writable by the caller's contract.
    unsafe { beat.write(value) };
    ST_CLOCK_OK
}

/// # Safety
/// `map` is null or a live map; `error` is null or valid for writes of `error_capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn st_clock_timeline_create(
    map: *const STClockMap,
    origin: STClockOrigin,
    error: *mut c_char,
    error_capacity: u32,
) -> *mut STClockTimeline {
    // SAFETY: the caller's contract for `error`.
    unsafe { error_text(error, error_capacity, "") };
    // SAFETY: a non-null `map` is a live, immutable map by the caller's contract.
    let result = match unsafe { map.as_ref() } {
        Some(map) => STClockTimeline::new(map, origin),
        None => Err(ClockError::MapRequired),
    };
    // SAFETY: the caller's contract for `error`.
    unsafe { into_handle(result, error, error_capacity) }
}

/// # Safety
/// `timeline` is null or came from [`st_clock_timeline_create`], has no reader left, and is not
/// used again.
#[no_mangle]
pub unsafe extern "C" fn st_clock_timeline_destroy(timeline: *mut STClockTimeline) {
    if !timeline.is_null() {
        // SAFETY: the pointer came from `Box::into_raw` in `st_clock_timeline_create` and
        // ownership returns here exactly once.
        drop(unsafe { Box::from_raw(timeline) });
    }
}

/// On error, `context` is left untouched.
///
/// # Safety
/// `timeline` is null or a live timeline; `context` is null or valid for a write.
#[no_mangle]
pub unsafe extern "C" fn st_clock_context(
    timeline: *const STClockTimeline,
    graph_frame: i64,
    upstream_latency_frames: i64,
    context: *mut STClockContext,
) -> i32 {
    // SAFETY: a non-null `timeline` is a live, immutable timeline by the caller's contract.
    let Some(timeline) = (unsafe { timeline.as_ref() }) else {
        return ST_CLOCK_INVALID;
    };
    if context.is_null() {
        return ST_CLOCK_INVALID;
    }
    match timeline.context(graph_frame, upstream_latency_frames) {
        Ok(value) => {
            // SAFETY: `context` is non-null and writable by the caller's contract.
            unsafe { context.write(value) };
            ST_CLOCK_OK
        }
        Err(error) => error.status(),
    }
}

/// On error, `span` is left untouched.
///
/// # Safety
/// `timeline` is null or a live timeline; `span` is null or valid for a write.
#[no_mangle]
pub unsafe extern "C" fn st_clock_span(
    timeline: *const STClockTimeline,
    graph_frame: i64,
    requested_frames: u32,
    upstream_latency_frames: i64,
    span: *mut STClockSpan,
) -> i32 {
    // Every argument error is ST_CLOCK_INVALID and all of them precede the domain checks, as in
    // the C++, so the order among them cannot be observed.
    // SAFETY: a non-null `timeline` is a live, immutable timeline by the caller's contract.
    let Some(timeline) = (unsafe { timeline.as_ref() }) else {
        return ST_CLOCK_INVALID;
    };
    if span.is_null() {
        return ST_CLOCK_INVALID;
    }
    match timeline.span(graph_frame, requested_frames, upstream_latency_frames) {
        Ok(value) => {
            // SAFETY: `span` is non-null and writable by the caller's contract.
            unsafe { span.write(value) };
            ST_CLOCK_OK
        }
        Err(error) => error.status(),
    }
}

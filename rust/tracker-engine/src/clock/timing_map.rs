//! Immutable tempo and meter map in quarter beats and seconds.
//!
//! Replaces `Sources/TrackerEngine/ClockTimingMap.hpp` and `ClockTimingMap.cpp`. The arithmetic
//! is the same, operation for operation, so compiled boundaries and query results are
//! bit-identical to the C++ engine (see [`super::long_double`] for the one target-dependent type).
//!
//! Construct on a control thread; construction validates and reports a [`TimingMapError`] where
//! the C++ constructor threw `std::invalid_argument`. Queries allocate nothing and never fail
//! other than by returning `None`. Input event order is significant: positions must be ascending
//! and unique, with an event at 0. There are no mutators or mutable views; inputs are copied.

use std::fmt;

use super::cxx_math;
use super::long_double::{CxxLongDouble, LongDouble};

/// A tempo change: `bpm` quarter notes per minute from quarter beat `beat` onwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoEvent {
    pub beat: f64,
    pub bpm: f64,
}

/// A meter change at quarter beat `beat`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterEvent {
    pub beat: f64,
    pub numerator: i32,
    pub denominator: i32,
}

/// A compiled tempo event with its start time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoSegment {
    pub beat: f64,
    pub seconds: f64,
    pub bpm: f64,
    pub seconds_per_quarter: f64,
}

/// A compiled meter event with its first bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterSegment {
    pub beat: f64,
    /// Human-facing bars are one-based.
    pub first_bar: u64,
    pub numerator: i32,
    pub denominator: i32,
    pub quarters_per_bar: f64,
}

/// Where a quarter beat falls within its bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarPosition {
    pub bar: u64,
    /// One-based denominator units, e.g. 1..5 in 5/8.
    pub notated_beat: f64,
    pub numerator: i32,
    pub denominator: i32,
    pub quarter_offset: f64,
    pub bar_start: f64,
    /// Exclusive. A meter event can shorten this bar.
    pub bar_end: f64,
    pub short_bar: bool,
}

/// Why a timing map could not be built. [`TimingMapError::message`] is the text the C++
/// constructor threw; the C API copies it to the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingMapError {
    /// No events, or more than [`TimingMap::MAX_EVENTS`].
    EventCount,
    /// The first event is not at quarter beat zero.
    FirstPosition,
    /// A position is not finite, not ascending, repeated, or outside 0..1000000.
    Positions,
    /// A tempo is not finite or outside 20..400.
    Tempo,
    /// Two tempo events start at the same time once rounded to double-precision seconds.
    TempoBoundariesTooClose,
    /// A numerator outside 1..16 or a denominator other than 2, 4, 8 or 16.
    Meter,
    /// The end of the beat domain has no distinct finite time.
    Domain,
}

impl TimingMapError {
    /// The message of the `std::invalid_argument` thrown by the C++ `TimingMap` constructor.
    pub const fn message(self) -> &'static str {
        match self {
            Self::EventCount => "A timing map requires 1..1024 events.",
            Self::FirstPosition => "A timing map must start at quarter beat zero.",
            Self::Positions => {
                "Timing positions must be finite, unique, ascending and within 0..1000000 quarter beats."
            }
            Self::Tempo => "Tempo must be finite and between 20 and 400 quarter notes per minute.",
            Self::TempoBoundariesTooClose => {
                "Tempo boundaries are too close to distinguish in double-precision seconds."
            }
            Self::Meter => "Meter requires numerator 1..16 and denominator 2, 4, 8 or 16.",
            Self::Domain => "Timing domain cannot be represented in finite seconds.",
        }
    }
}

impl fmt::Display for TimingMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for TimingMapError {}

/// Validated, compiled tempo and meter events. See the module documentation.
#[derive(Clone, Debug)]
pub struct TimingMap {
    tempos: Vec<TempoSegment>,
    meters: Vec<MeterSegment>,
    max_seconds: f64,
    max_bar: u64,
}

fn valid_beat(beat: f64) -> bool {
    beat.is_finite() && (0.0..=TimingMap::MAX_BEAT).contains(&beat)
}

/// The checks shared by tempo and meter events, on their positions only.
fn validate_positions(
    count: usize,
    mut beats: impl Iterator<Item = f64>,
) -> Result<(), TimingMapError> {
    if count == 0 || count > TimingMap::MAX_EVENTS {
        return Err(TimingMapError::EventCount);
    }
    let mut beats = beats.by_ref().peekable();
    // A NaN first position is "not zero" here, as in the C++ comparison.
    if beats.peek().is_some_and(|first| *first != 0.0) {
        return Err(TimingMapError::FirstPosition);
    }
    let mut previous = -1.0;
    for beat in beats {
        if !valid_beat(beat) || beat <= previous {
            return Err(TimingMapError::Positions);
        }
        previous = beat;
    }
    Ok(())
}

/// `compileTempos`, generic over the C++ `long double` so both variants can be tested anywhere.
fn compile_tempos<L: LongDouble>(
    events: &[TempoEvent],
) -> Result<Vec<TempoSegment>, TimingMapError> {
    validate_positions(events.len(), events.iter().map(|event| event.beat))?;
    let mut segments: Vec<TempoSegment> = Vec::with_capacity(events.len());
    let mut seconds = L::from_f64(0.0);
    let mut correction = L::from_f64(0.0);
    let mut previous: Option<&TempoEvent> = None;
    for event in events {
        if !event.bpm.is_finite()
            || event.bpm < TimingMap::MIN_BPM
            || event.bpm > TimingMap::MAX_BPM
        {
            return Err(TimingMapError::Tempo);
        }
        if let Some(previous) = previous {
            // Compensation also helps targets where long double is only 64-bit.
            let duration = (L::from_f64(event.beat) - L::from_f64(previous.beat))
                * L::from_f64(60.0)
                / L::from_f64(previous.bpm);
            let adjusted = duration - correction;
            let next = seconds + adjusted;
            correction = (next - seconds) - adjusted;
            seconds = next;
        }
        let rounded = seconds.to_f64();
        if !rounded.is_finite() || segments.last().is_some_and(|last| rounded <= last.seconds) {
            return Err(TimingMapError::TempoBoundariesTooClose);
        }
        segments.push(TempoSegment {
            // A negative zero position is stored as plain zero.
            beat: if event.beat == 0.0 { 0.0 } else { event.beat },
            seconds: rounded,
            bpm: event.bpm,
            seconds_per_quarter: 60.0 / event.bpm,
        });
        previous = Some(event);
    }
    Ok(segments)
}

/// A rounded start computed this way is the authoritative floating-point bar boundary.
/// Comparing against it avoids epsilon-snapping a deliberate short bar.
fn bar_start(segment: &MeterSegment, offset: u64) -> f64 {
    (offset as f64).mul_add(segment.quarters_per_bar, segment.beat)
}

/// Zero-based index, within `segment`, of the bar containing `beat` (which is not before it).
fn bar_offset(segment: &MeterSegment, beat: f64) -> u64 {
    let mut offset = ((beat - segment.beat) / segment.quarters_per_bar).floor() as u64;
    if offset != 0 && beat < bar_start(segment, offset) {
        offset -= 1;
    }
    if beat >= bar_start(segment, offset + 1) {
        offset += 1;
    }
    offset
}

fn compile_meters(events: &[MeterEvent]) -> Result<Vec<MeterSegment>, TimingMapError> {
    validate_positions(events.len(), events.iter().map(|event| event.beat))?;
    let mut segments: Vec<MeterSegment> = Vec::with_capacity(events.len());
    let mut first_bar: u64 = 1;
    for event in events {
        if event.numerator < 1
            || event.numerator > 16
            || !matches!(event.denominator, 2 | 4 | 8 | 16)
        {
            return Err(TimingMapError::Meter);
        }
        if let Some(previous) = segments.last() {
            let offset = bar_offset(previous, event.beat);
            // A meter change inside a bar cuts that bar short; the new meter starts the next one.
            first_bar = previous.first_bar
                + offset
                + if event.beat == bar_start(previous, offset) {
                    0
                } else {
                    1
                };
        }
        segments.push(MeterSegment {
            beat: if event.beat == 0.0 { 0.0 } else { event.beat },
            first_bar,
            numerator: event.numerator,
            denominator: event.denominator,
            quarters_per_bar: f64::from(event.numerator) * 4.0 / f64::from(event.denominator),
        });
    }
    Ok(segments)
}

/// Index of the last segment whose key is not after the query (`std::upper_bound` minus one).
/// `is_after_query` must be false for the first segment, which holds for every in-domain query
/// because compiled maps start at beat, second and bar zero/one.
fn last_segment_at<S>(segments: &[S], is_after_query: impl Fn(&S) -> bool) -> usize {
    segments
        .partition_point(|segment| !is_after_query(segment))
        .saturating_sub(1)
}

fn seconds_in_segment(segment: &TempoSegment, beat: f64) -> f64 {
    (beat - segment.beat).mul_add(segment.seconds_per_quarter, segment.seconds)
}

impl TimingMap {
    /// Most tempo events, and most meter events, in one map.
    pub const MAX_EVENTS: usize = 1024;
    /// Last quarter beat of the (closed) query domain.
    pub const MAX_BEAT: f64 = 1_000_000.0;
    pub const MIN_BPM: f64 = 20.0;
    pub const MAX_BPM: f64 = 400.0;

    /// Validates and compiles the events. Tempo errors are reported before meter errors.
    pub fn new(tempos: &[TempoEvent], meters: &[MeterEvent]) -> Result<Self, TimingMapError> {
        Self::with_long_double::<CxxLongDouble>(tempos, meters)
    }

    /// [`Self::new`] with an explicit C++ `long double`, so tests can cover every target's.
    fn with_long_double<L: LongDouble>(
        tempos: &[TempoEvent],
        meters: &[MeterEvent],
    ) -> Result<Self, TimingMapError> {
        let tempos = compile_tempos::<L>(tempos)?;
        let meters = compile_meters(meters)?;
        // Both compilers reject empty input.
        let (Some(last_tempo), Some(last_meter)) = (tempos.last(), meters.last()) else {
            return Err(TimingMapError::EventCount);
        };
        let max_seconds = seconds_in_segment(last_tempo, Self::MAX_BEAT);
        let max_bar = last_meter.first_bar + bar_offset(last_meter, Self::MAX_BEAT);
        if !max_seconds.is_finite()
            || max_seconds < last_tempo.seconds
            || (last_tempo.beat < Self::MAX_BEAT && max_seconds == last_tempo.seconds)
        {
            return Err(TimingMapError::Domain);
        }
        Ok(Self {
            tempos,
            meters,
            max_seconds,
            max_bar,
        })
    }

    /// Compiled tempo events, ascending; never empty.
    pub fn tempos(&self) -> &[TempoSegment] {
        &self.tempos
    }

    /// Compiled meter events, ascending; never empty.
    pub fn meters(&self) -> &[MeterSegment] {
        &self.meters
    }

    /// Seconds at [`Self::MAX_BEAT`].
    pub fn max_seconds(&self) -> f64 {
        self.max_seconds
    }

    // Closed query domain: 0 <= quarter beat <= MAX_BEAT, and corresponding seconds.
    // Invalid, nonfinite and out-of-domain queries return None.

    pub fn seconds_at_beat(&self, beat: f64) -> Option<f64> {
        if !valid_beat(beat) {
            return None;
        }
        if beat == Self::MAX_BEAT {
            return Some(self.max_seconds);
        }
        let index = last_segment_at(&self.tempos, |segment| beat < segment.beat);
        let mut result = seconds_in_segment(self.tempos.get(index)?, beat);
        // Adjacent segments share their stored boundary despite prefix rounding.
        if let Some(next) = self.tempos.get(index + 1) {
            result = cxx_math::min(result, next.seconds);
        }
        Some(cxx_math::min(result, self.max_seconds))
    }

    pub fn beat_at_seconds(&self, seconds: f64) -> Option<f64> {
        if !seconds.is_finite() || seconds < 0.0 || seconds > self.max_seconds {
            return None;
        }
        if seconds == self.max_seconds {
            return Some(Self::MAX_BEAT);
        }
        let index = last_segment_at(&self.tempos, |segment| seconds < segment.seconds);
        let segment = self.tempos.get(index)?;
        if seconds == segment.seconds {
            return Some(segment.beat);
        }
        let mut result = (seconds - segment.seconds).mul_add(segment.bpm / 60.0, segment.beat);
        if let Some(next) = self.tempos.get(index + 1) {
            result = cxx_math::min(result, next.beat);
        }
        Some(cxx_math::min(result, Self::MAX_BEAT))
    }

    pub fn tempo_at_beat(&self, beat: f64) -> Option<f64> {
        if !valid_beat(beat) {
            return None;
        }
        let index = last_segment_at(&self.tempos, |segment| beat < segment.beat);
        Some(self.tempos.get(index)?.bpm)
    }

    pub fn bar_at_beat(&self, beat: f64) -> Option<BarPosition> {
        if !valid_beat(beat) {
            return None;
        }
        let index = last_segment_at(&self.meters, |segment| beat < segment.beat);
        let segment = self.meters.get(index)?;
        let offset = bar_offset(segment, beat);
        let start = bar_start(segment, offset);
        let nominal_end = bar_start(segment, offset + 1);
        let end = match self.meters.get(index + 1) {
            Some(next) => cxx_math::min(nominal_end, next.beat),
            None => nominal_end,
        };
        let quarter_offset = beat - start;
        let denominator = f64::from(segment.denominator);
        let past_last_beat = f64::from(segment.numerator) + 1.0;
        let mut notated_beat = quarter_offset.mul_add(denominator / 4.0, 1.0);
        if notated_beat >= past_last_beat {
            notated_beat = cxx_math::next_below(past_last_beat);
        }
        // Adding the one-based label can round an interior point up to a short
        // bar's exclusive end. Keep the label readable back inside its own bar.
        // One label ULP suffices with the validated dyadic denominator units.
        if notated_beat > 1.0 && (notated_beat - 1.0).mul_add(4.0 / denominator, start) >= end {
            notated_beat = cxx_math::next_below(notated_beat);
        }
        debug_assert!((notated_beat - 1.0).mul_add(4.0 / denominator, start) < end);
        Some(BarPosition {
            bar: segment.first_bar + offset,
            notated_beat,
            numerator: segment.numerator,
            denominator: segment.denominator,
            quarter_offset,
            bar_start: start,
            bar_end: end,
            short_bar: end < nominal_end,
        })
    }

    /// Rejects a beat at/after this bar's exclusive end, including the removed
    /// part of a short bar. Use the following bar's beat 1 for that boundary.
    pub fn beat_at_bar_position(&self, bar: u64, notated_beat: f64) -> Option<f64> {
        if bar < 1 || bar > self.max_bar || !notated_beat.is_finite() || notated_beat < 1.0 {
            return None;
        }
        let index = last_segment_at(&self.meters, |segment| bar < segment.first_bar);
        let segment = self.meters.get(index)?;
        if notated_beat >= f64::from(segment.numerator) + 1.0 {
            return None;
        }
        let offset = bar - segment.first_bar;
        let start = bar_start(segment, offset);
        let nominal_end = bar_start(segment, offset + 1);
        let end = match self.meters.get(index + 1) {
            Some(next) => cxx_math::min(nominal_end, next.beat),
            None => nominal_end,
        };
        let beat = (notated_beat - 1.0).mul_add(4.0 / f64::from(segment.denominator), start);
        if !valid_beat(beat) || beat >= end {
            return None;
        }
        Some(beat)
    }
}

#[cfg(test)]
mod tests {
    use super::super::x87::Extended;
    use super::*;

    fn tempo(beat: f64, bpm: f64) -> TempoEvent {
        TempoEvent { beat, bpm }
    }

    fn meter(beat: f64, numerator: i32, denominator: i32) -> MeterEvent {
        MeterEvent {
            beat,
            numerator,
            denominator,
        }
    }

    fn map(tempos: &[TempoEvent], meters: &[MeterEvent]) -> TimingMap {
        TimingMap::new(tempos, meters).expect("valid map")
    }

    fn error(tempos: &[TempoEvent], meters: &[MeterEvent]) -> TimingMapError {
        TimingMap::new(tempos, meters).expect_err("invalid map")
    }

    const COMMON: [MeterEvent; 1] = [MeterEvent {
        beat: 0.0,
        numerator: 4,
        denominator: 4,
    }];
    const STEADY: [TempoEvent; 1] = [TempoEvent {
        beat: 0.0,
        bpm: 120.0,
    }];

    #[test]
    fn validation_reports_the_cxx_errors_in_the_cxx_order() {
        assert_eq!(error(&[], &COMMON), TimingMapError::EventCount);
        assert_eq!(error(&STEADY, &[]), TimingMapError::EventCount);
        let many = vec![tempo(0.0, 120.0); 1025];
        assert_eq!(error(&many, &COMMON), TimingMapError::EventCount);
        assert_eq!(
            error(&[tempo(1.0, 120.0)], &COMMON),
            TimingMapError::FirstPosition
        );
        assert_eq!(
            error(&[tempo(f64::NAN, 120.0)], &COMMON),
            TimingMapError::FirstPosition
        );
        for bad in [f64::NAN, f64::INFINITY, 1_000_000.5, 0.0, -0.0, -1.0] {
            assert_eq!(
                error(&[tempo(0.0, 120.0), tempo(bad, 120.0)], &COMMON),
                TimingMapError::Positions
            );
        }
        assert_eq!(
            error(
                &[tempo(0.0, 120.0), tempo(2.0, 60.0), tempo(1.0, 90.0)],
                &COMMON
            ),
            TimingMapError::Positions
        );
        for bad in [
            f64::NAN,
            f64::INFINITY,
            -f64::INFINITY,
            19.999,
            400.001,
            0.0,
            -120.0,
        ] {
            assert_eq!(error(&[tempo(0.0, bad)], &COMMON), TimingMapError::Tempo);
        }
        // Positions are checked for the whole list before any tempo value.
        assert_eq!(
            error(&[tempo(0.0, 0.0), tempo(-1.0, 120.0)], &COMMON),
            TimingMapError::Positions
        );
        // Tempo errors come before meter errors.
        assert_eq!(
            error(&[tempo(0.0, 0.0)], &[meter(0.0, 5, 3)]),
            TimingMapError::Tempo
        );
        for (numerator, denominator) in [
            (0, 4),
            (17, 4),
            (-1, 4),
            (4, 0),
            (4, 3),
            (4, 32),
            (4, -4),
            (4, 1),
        ] {
            assert_eq!(
                error(&STEADY, &[meter(0.0, numerator, denominator)]),
                TimingMapError::Meter
            );
        }
        assert_eq!(
            error(&STEADY, &[meter(0.5, 4, 4)]),
            TimingMapError::FirstPosition
        );
        assert_eq!(
            error(&STEADY, &[meter(0.0, 4, 4), meter(0.0, 3, 4)]),
            TimingMapError::Positions
        );
        assert_eq!(
            TimingMapError::Positions.to_string(),
            "Timing positions must be finite, unique, ascending and within 0..1000000 quarter beats."
        );
    }

    #[test]
    fn boundaries_must_be_distinct_in_seconds() {
        // Denormal spacing cannot be told apart in seconds.
        assert_eq!(
            error(
                &[tempo(0.0, 120.0), tempo(f64::from_bits(1), 100.0)],
                &COMMON
            ),
            TimingMapError::TempoBoundariesTooClose
        );
        // Neither can adjacent doubles far into a fast section.
        let late = 900_000.0f64;
        assert_eq!(
            error(
                &[
                    tempo(0.0, 20.0),
                    tempo(late, 400.0),
                    tempo(late.next_up(), 20.0)
                ],
                &COMMON
            ),
            TimingMapError::TempoBoundariesTooClose
        );
        // A last tempo so close to the end of the domain that the end has no later time.
        assert_eq!(
            error(
                &[tempo(0.0, 20.0), tempo(1_000_000.0f64.next_down(), 400.0)],
                &COMMON
            ),
            TimingMapError::Domain
        );
    }

    #[test]
    fn limits_are_inclusive() {
        let many: Vec<TempoEvent> = (0..1024)
            .map(|index| tempo(f64::from(index), 20.0 + f64::from(index % 381)))
            .collect();
        let meters: Vec<MeterEvent> = (0..1024)
            .map(|index| meter(f64::from(index), 1 + index % 16, 2 << (index % 4)))
            .collect();
        let full = map(&many, &meters);
        assert_eq!(full.tempos().len(), 1024);
        assert_eq!(full.meters().len(), 1024);
        // An event exactly at the end of the domain is allowed and leaves an empty last segment.
        let at_end = map(
            &[tempo(0.0, 20.0), tempo(1_000_000.0, 400.0)],
            &[meter(0.0, 16, 2), meter(1_000_000.0, 1, 16)],
        );
        assert_eq!(at_end.max_seconds(), 3_000_000.0);
        assert_eq!(at_end.seconds_at_beat(1_000_000.0), Some(3_000_000.0));
        assert_eq!(at_end.beat_at_seconds(3_000_000.0), Some(1_000_000.0));
        assert_eq!(at_end.tempo_at_beat(1_000_000.0), Some(400.0));
        assert_eq!(at_end.tempo_at_beat(999_999.999), Some(20.0));
        let last = at_end.bar_at_beat(1_000_000.0).unwrap();
        assert_eq!(
            (
                last.bar,
                last.numerator,
                last.denominator,
                last.notated_beat
            ),
            (31_251, 1, 16, 1.0)
        );
        assert_eq!(
            (last.bar_start, last.bar_end, last.short_bar),
            (1_000_000.0, 1_000_000.25, false)
        );
        assert_eq!(at_end.beat_at_bar_position(31_251, 1.0), Some(1_000_000.0));
        assert_eq!(at_end.beat_at_bar_position(31_252, 1.0), None);
        // Negative zero is a valid first position and is stored as plain zero.
        let negative_zero = map(&[tempo(-0.0, 120.0)], &[meter(-0.0, 4, 4)]);
        assert!(negative_zero.tempos()[0].beat.is_sign_positive());
        assert!(negative_zero.meters()[0].beat.is_sign_positive());
        assert_eq!(negative_zero.seconds_at_beat(-0.0), Some(0.0));
        assert_eq!(negative_zero.bar_at_beat(-0.0).unwrap().bar, 1);
    }

    #[test]
    fn queries_reject_values_outside_the_closed_domain() {
        let map = map(
            &[tempo(0.0, 120.0), tempo(4.0, 60.0)],
            &[meter(0.0, 4, 4), meter(6.0, 5, 8)],
        );
        for bad in [
            f64::NAN,
            f64::INFINITY,
            -f64::INFINITY,
            -1e-300,
            1_000_000.000_000_1,
        ] {
            assert_eq!(map.seconds_at_beat(bad), None);
            assert_eq!(map.tempo_at_beat(bad), None);
            assert_eq!(map.bar_at_beat(bad), None);
        }
        for bad in [
            f64::NAN,
            f64::INFINITY,
            -1e-300,
            map.max_seconds().next_up(),
        ] {
            assert_eq!(map.beat_at_seconds(bad), None);
        }
        assert_eq!(map.max_seconds(), 2.0 + 999_996.0);
        assert_eq!(map.seconds_at_beat(0.0), Some(0.0));
        assert_eq!(map.seconds_at_beat(4.0), Some(2.0));
        assert_eq!(map.seconds_at_beat(6.0), Some(4.0));
        assert_eq!(map.beat_at_seconds(4.0), Some(6.0));
        assert_eq!(map.beat_at_seconds(map.max_seconds()), Some(1_000_000.0));
        assert_eq!(map.tempo_at_beat(4.0f64.next_down()), Some(120.0));
        assert_eq!(map.tempo_at_beat(4.0), Some(60.0));
        for bad in [f64::NAN, f64::INFINITY, 0.999_999, 5.0, 1e300] {
            assert_eq!(map.beat_at_bar_position(1, bad), None);
        }
        assert_eq!(map.beat_at_bar_position(0, 1.0), None);
        assert_eq!(map.beat_at_bar_position(u64::MAX, 1.0), None);
        assert_eq!(map.beat_at_bar_position(1, 1.0), Some(0.0));
        // The last label of the bar: (5 - 4 ulp) - 1 is exact, two representable steps below 4.
        assert_eq!(
            map.beat_at_bar_position(1, 5.0f64.next_down()),
            Some(4.0f64.next_down().next_down())
        );
        // The last addressable bar is the one containing beat 1000000.
        let last_bar = map.bar_at_beat(1_000_000.0).unwrap().bar;
        assert!(map.beat_at_bar_position(last_bar, 1.0).is_some());
        assert_eq!(map.beat_at_bar_position(last_bar + 1, 1.0), None);
    }

    #[test]
    fn a_meter_change_inside_a_bar_shortens_it() {
        // 4/4 from 0; 5/8 from beat 6, half way through bar 2 (beats 4..8).
        let map = map(&STEADY, &[meter(0.0, 4, 4), meter(6.0, 5, 8)]);
        assert_eq!(map.meters()[1].first_bar, 3);
        let short = map.bar_at_beat(5.0).unwrap();
        assert_eq!(
            (short.bar, short.bar_start, short.bar_end, short.short_bar),
            (2, 4.0, 6.0, true)
        );
        assert_eq!((short.notated_beat, short.quarter_offset), (2.0, 1.0));
        let next = map.bar_at_beat(6.0).unwrap();
        assert_eq!(
            (
                next.bar,
                next.numerator,
                next.denominator,
                next.bar_start,
                next.bar_end
            ),
            (3, 5, 8, 6.0, 8.5)
        );
        assert!(!next.short_bar);
        assert_eq!(map.bar_at_beat(8.25).unwrap().notated_beat, 5.5);
        // The removed part of the short bar is not addressable; the next bar's beat 1 is.
        assert_eq!(map.beat_at_bar_position(2, 2.0), Some(5.0));
        assert_eq!(map.beat_at_bar_position(2, 3.0), None);
        assert_eq!(map.beat_at_bar_position(2, 2.75), Some(5.75));
        // A label just below the cut rounds onto it, so it is rejected as well.
        assert_eq!(map.beat_at_bar_position(2, 3.0f64.next_down()), None);
        assert_eq!(map.beat_at_bar_position(3, 1.0), Some(6.0));
        assert_eq!(map.beat_at_bar_position(4, 1.0), Some(8.5));
        // A change exactly on a bar line does not add a bar.
        let aligned = self::map(&STEADY, &[meter(0.0, 4, 4), meter(8.0, 3, 4)]);
        assert_eq!(aligned.meters()[1].first_bar, 3);
    }

    #[test]
    fn labels_stay_inside_their_bar() {
        // The bar is cut just below a label boundary: the rounded label must read back inside it.
        let cut = 2.0f64.next_down();
        let map = map(&STEADY, &[meter(0.0, 4, 4), meter(cut, 4, 4)]);
        let before = map.bar_at_beat(cut.next_down()).unwrap();
        assert!(before.short_bar);
        assert!(before.notated_beat < 3.0);
        let back = map
            .beat_at_bar_position(before.bar, before.notated_beat)
            .unwrap();
        assert!(back < before.bar_end);
        // In a full bar the last representable position keeps a label below numerator + 1.
        let full = self::map(&STEADY, &[meter(0.0, 7, 16)]);
        let last = full.bar_at_beat(1.75f64.next_down()).unwrap();
        assert_eq!(last.bar, 1);
        assert!(last.notated_beat < 8.0);
        assert_eq!(full.bar_at_beat(1.75).unwrap().bar, 2);
    }

    /// SplitMix64, as in the C++ reference program described at `matches_the_cxx_timing_map`.
    struct Generator(u64);

    impl Generator {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / 9_007_199_254_740_992.0
        }
        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
        fn step(&mut self, style: u64) -> f64 {
            match style {
                0 => 0.25 * (1 + self.below(16)) as f64,
                1 => self.unit() * 4.0,
                2 => self.unit() * 400.0,
                3 => 1e-9 + self.unit() * 1e-4,
                _ => self.unit() * 30000.0,
            }
        }
        fn around(&mut self, value: f64) -> f64 {
            match self.below(3) {
                0 => value,
                1 => value.next_up(),
                _ => value.next_down(),
            }
        }
    }

    /// FNV-1a over little-endian 64-bit words.
    struct Digest(u64);

    impl Digest {
        fn mix(&mut self, value: u64) {
            for byte in value.to_le_bytes() {
                self.0 ^= u64::from(byte);
                self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        fn mix_f64(&mut self, value: f64) {
            self.mix(value.to_bits());
        }
        fn mix_option(&mut self, value: Option<f64>) {
            match value {
                Some(value) => {
                    self.mix(1);
                    self.mix_f64(value);
                }
                None => self.mix(0),
            }
        }
    }

    /// Thousands of pseudo-random maps and queries, digested exactly as the reference program
    /// digests them. The order of generator calls is part of the contract with that program.
    /// Returns the digest and the number of maps that were valid.
    fn digest_of_random_maps<L: LongDouble>(cases: u64) -> (u64, u32) {
        let mut r = Generator(0x5eed);
        let mut digest = Digest(0xcbf2_9ce4_8422_2325);
        let mut valid = 0;
        for index in 0..cases {
            let nt = 1 + r.below(12);
            let style = r.below(5);
            let mut tempos = Vec::new();
            let mut position = 0.0;
            for i in 0..nt {
                if i != 0 {
                    position += r.step(style);
                }
                let bpm = if r.below(2) == 0 {
                    (20 + r.below(381)) as f64
                } else {
                    20.0 + 380.0 * r.unit()
                };
                tempos.push(tempo(position, bpm));
            }
            let nm = 1 + r.below(8);
            let meter_style = r.below(5);
            let mut meters: Vec<MeterEvent> = Vec::new();
            position = 0.0;
            for i in 0..nm {
                if i != 0 {
                    position += r.step(meter_style);
                    if r.below(100) < 40 {
                        let previous = meters[meters.len() - 1].beat;
                        if let Some(event) = tempos.iter().find(|event| event.beat > previous) {
                            position = event.beat;
                        }
                    }
                }
                let numerator = (1 + r.below(16)) as i32;
                let denominator = 2 << r.below(4);
                meters.push(meter(position, numerator, denominator));
            }
            let (last_tempo, last_meter) = (tempos.len() - 1, meters.len() - 1);
            if r.below(100) < 6 {
                match r.below(6) {
                    0 => tempos[0].beat = 0.5,
                    1 => tempos[last_tempo].bpm = 500.0,
                    2 => meters[last_meter].denominator = 3,
                    3 if nt > 1 => tempos[last_tempo].beat = tempos[last_tempo - 1].beat,
                    4 if nt > 1 => tempos[last_tempo].beat = tempos[last_tempo - 1].beat.next_up(),
                    3 | 4 => {}
                    _ => tempos[last_tempo].beat = 1_000_000.0f64.next_down(),
                }
            }
            digest.mix(index);
            let map = match TimingMap::with_long_double::<L>(&tempos, &meters) {
                Ok(map) => map,
                Err(error) => {
                    digest.mix(1000 + error as u64);
                    continue;
                }
            };
            valid += 1;
            for s in map.tempos() {
                digest.mix_f64(s.beat);
                digest.mix_f64(s.seconds);
                digest.mix_f64(s.bpm);
                digest.mix_f64(s.seconds_per_quarter);
            }
            for s in map.meters() {
                digest.mix_f64(s.beat);
                digest.mix(s.first_bar);
                digest.mix(s.numerator as u64);
                digest.mix(s.denominator as u64);
                digest.mix_f64(s.quarters_per_bar);
            }
            digest.mix_f64(map.max_seconds());
            let last = cxx_math::max(tempos[last_tempo].beat, meters[last_meter].beat);
            for _ in 0..24 {
                let beat = match r.below(8) {
                    0 => r.unit() * 1_000_000.0,
                    1 => 1_000_000.0,
                    2 => {
                        let event = tempos[r.below(nt) as usize];
                        r.around(event.beat)
                    }
                    3 => {
                        let event = meters[r.below(nm) as usize];
                        r.around(event.beat)
                    }
                    4 => r.unit() * (last + 8.0),
                    5 => r.below((last * 4.0) as u64 + 32) as f64 * 0.25,
                    6 => [f64::NAN, -1.0, 1_000_001.0][r.below(3) as usize],
                    _ => r.unit() * (last * 1.25 + 4.0),
                };
                let seconds = map.seconds_at_beat(beat);
                digest.mix_option(seconds);
                digest.mix_option(map.tempo_at_beat(beat));
                if let Some(seconds) = seconds {
                    digest.mix_option(map.beat_at_seconds(seconds));
                }
                digest.mix_option(map.beat_at_seconds(r.unit() * map.max_seconds()));
                let Some(bar) = map.bar_at_beat(beat) else {
                    digest.mix(0);
                    continue;
                };
                digest.mix(1);
                digest.mix(bar.bar);
                digest.mix_f64(bar.notated_beat);
                digest.mix(bar.numerator as u64);
                digest.mix(bar.denominator as u64);
                digest.mix_f64(bar.quarter_offset);
                digest.mix_f64(bar.bar_start);
                digest.mix_f64(bar.bar_end);
                digest.mix(bar.short_bar as u64);
                digest.mix_option(map.beat_at_bar_position(bar.bar, bar.notated_beat));
                digest.mix_option(map.beat_at_bar_position(bar.bar, 1.0));
                digest.mix_option(map.beat_at_bar_position(bar.bar + 1, 1.0));
                let label = f64::from(bar.numerator) + r.unit();
                digest.mix_option(map.beat_at_bar_position(bar.bar, label));
                let label = 1.0 + r.unit() * f64::from(bar.numerator);
                digest.mix_option(map.beat_at_bar_position(bar.bar, label));
                let other_bar = r.below(bar.bar + 3);
                digest.mix_option(map.beat_at_bar_position(other_bar, 1.0 + r.unit() * 17.0));
            }
            println!("{index} {:016x}", digest.0);
        }
        (digest.0, valid)
    }

    /// Differential check against the C++ `TimingMap`, including `beatAtBarPosition` and the
    /// `BarPosition` fields that the C API does not expose. The expected digests were printed by
    /// a C++ program with this same generator and digest, linked with the original
    /// `ClockTimingMap.cpp`: once as built for x86-64 (80-bit `long double`) and once with
    /// `-mlong-double-64` (the `long double` of arm64 macOS).
    #[test]
    fn matches_the_cxx_timing_map() {
        assert_eq!(
            digest_of_random_maps::<Extended>(4000),
            (0x6c93_b848_a06b_410b, 3804)
        );
        assert_eq!(
            digest_of_random_maps::<f64>(4000),
            (0x0642_6102_f3d5_0660, 3803)
        );
    }

    #[test]
    fn error_discriminants_are_the_reference_program_codes() {
        // `matches_the_cxx_timing_map` digests errors by position in this list.
        let order = [
            TimingMapError::EventCount,
            TimingMapError::FirstPosition,
            TimingMapError::Positions,
            TimingMapError::Tempo,
            TimingMapError::TempoBoundariesTooClose,
            TimingMapError::Meter,
            TimingMapError::Domain,
        ];
        for (index, error) in order.into_iter().enumerate() {
            assert_eq!(error as usize, index);
        }
    }
}

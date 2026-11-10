//! The render clock that MIDI is scheduled against: the relation between the audio graph's frame
//! counter and device time. Pure arithmetic for the audio thread: no allocation, no clock read.

/// The time of an audio render block as the audio device reports it. A field is `None` when the
/// device did not vouch for it (offline rendering has no device time at all).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RenderStamp {
    /// Position of the block's first frame on the graph's frame counter.
    pub sample_time: Option<f64>,
    /// Device time of the block's first frame, in ticks ([`crate::MidiClock`] nanoseconds in the
    /// host). Zero is not a time.
    pub host_time: Option<u64>,
}

impl RenderStamp {
    /// A stamp with both representations.
    pub fn new(sample_time: f64, host_time: u64) -> Self {
        RenderStamp { sample_time: Some(sample_time), host_time: Some(host_time) }
    }

    /// The device time to schedule against, or 0 ("send now") when the stamp has none. Garbage is
    /// never scheduled: a stamp without a vouched device time must not place MIDI in the far future.
    pub fn host_time_or_now(&self) -> u64 {
        self.host_time.unwrap_or(0)
    }
}

/// Anchors look-ahead MIDI to the master render block: only a stamp with both a whole frame
/// position and a device time is an anchor, and every block replaces the previous one.
#[derive(Debug, Clone, Copy)]
pub struct Timebase {
    frame: i64,
    ticks: u64,
    valid: bool,
    ticks_per_frame: f64,
    maximum_distance: i64,
}

impl Timebase {
    /// Frames further than this from the anchor are not converted: the anchor is too old to trust.
    pub const DEFAULT_MAXIMUM_DISTANCE: i64 = 262_144;

    /// A clock with `ticks_per_frame` device ticks per audio frame (1e9 / sample rate in the host).
    pub fn new(ticks_per_frame: f64) -> Self {
        Self::with_maximum_distance(ticks_per_frame, Self::DEFAULT_MAXIMUM_DISTANCE)
    }

    /// Like [`new`](Self::new) with another bound on how far from the anchor times are converted.
    pub fn with_maximum_distance(ticks_per_frame: f64, maximum_distance: i64) -> Self {
        Timebase { frame: 0, ticks: 0, valid: false, ticks_per_frame, maximum_distance }
    }

    /// Device ticks per frame.
    pub fn ticks_per_frame(&self) -> f64 {
        self.ticks_per_frame
    }

    /// True while an anchor is held.
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// Drops the anchor (the graph restarted).
    pub fn reset(&mut self) {
        self.valid = false;
    }

    /// Takes the stamp of a master render block as the new anchor. The old anchor is dropped first,
    /// so a malformed stamp can never leave a stale clock in place.
    pub fn observe(&mut self, stamp: &RenderStamp) {
        self.valid = false;
        let (Some(sample_time), Some(host_time)) = (stamp.sample_time, stamp.host_time) else {
            return;
        };
        let usable = host_time > 0
            && sample_time.is_finite()
            && sample_time.round() == sample_time
            && sample_time.abs() < (1u64 << 52) as f64
            && self.ticks_per_frame.is_finite()
            && self.ticks_per_frame > 0.0
            && self.maximum_distance > 0;
        if usable {
            self.frame = sample_time as i64;
            self.ticks = host_time;
            self.valid = true;
        }
    }

    /// The graph frame that plays at device time `target`. A source with its own frame origin still
    /// reports the device time of its block, which identifies the same physical moment.
    pub fn graph_frame_at(&self, target: u64) -> Option<i64> {
        if !self.valid || target == 0 {
            return None;
        }
        let delta = if target >= self.ticks { (target - self.ticks) as f64 } else { -((self.ticks - target) as f64) };
        let frames = (delta / self.ticks_per_frame).round();
        if !frames.is_finite() || frames.abs() > self.maximum_distance as f64 {
            return None;
        }
        self.frame.checked_add(frames as i64)
    }

    /// The device time at which graph frame `target` plays.
    pub fn timestamp_at(&self, target: i64) -> Option<u64> {
        if !self.valid {
            return None;
        }
        let delta = target.checked_sub(self.frame)?;
        if delta < -self.maximum_distance || delta > self.maximum_distance {
            return None;
        }
        let offset = (delta as f64 * self.ticks_per_frame).round();
        if !offset.is_finite() || offset.abs() >= u64::MAX as f64 {
            return None;
        }
        if offset < 0.0 {
            let amount = (-offset) as u64;
            return (amount < self.ticks).then(|| self.ticks - amount);
        }
        self.ticks.checked_add(offset as u64).filter(|&result| result != 0)
    }
}

#[cfg(test)]
mod tests {
    //! Ported from tests/AudioMIDITimebaseTests.swift.
    use super::*;

    #[test]
    fn host_time_without_a_vouched_value_means_now() {
        assert_eq!(RenderStamp { sample_time: Some(0.0), host_time: None }.host_time_or_now(), 0, "an unflagged host time must not schedule MIDI in the distant future");
        assert_eq!(RenderStamp::default().host_time_or_now(), 0);
        assert_eq!(RenderStamp::new(0.0, 123_456).host_time_or_now(), 123_456, "a valid live host clock is kept");
    }

    #[test]
    fn master_clock_authority_lookahead_latency_and_invalidation() {
        let mut clock = Timebase::new(500.0);
        assert_eq!(clock.timestamp_at(0), None, "unanchored clock");
        clock.observe(&RenderStamp::new(4096.0, 1_000_000_000));
        assert_eq!(clock.timestamp_at(4096), Some(1_000_000_000), "current frame");
        assert_eq!(clock.timestamp_at(8193), Some(1_002_048_500), "look-ahead stays scheduled in the future");
        assert_eq!(clock.timestamp_at(4032), Some(999_968_000), "latency offset");
        assert_eq!(clock.timestamp_at(4096 + 262_145), None, "stale bound");
        clock.observe(&RenderStamp::new(8192.0, 2_000_000_000));
        assert_eq!(clock.timestamp_at(8193), Some(2_000_000_500), "new master clock");
        let invalid = [
            RenderStamp { sample_time: None, host_time: None },
            RenderStamp { sample_time: Some(0.0), host_time: None },
            RenderStamp { sample_time: None, host_time: Some(10) },
            RenderStamp::new(0.0, 0),
            RenderStamp::new(0.5, 10),
            RenderStamp::new(f64::NAN, 10),
            RenderStamp::new(f64::INFINITY, 10),
        ];
        for stamp in invalid {
            clock.observe(&RenderStamp::new(0.0, 100));
            clock.observe(&stamp);
            assert_eq!(clock.timestamp_at(0), None, "an invalid stamp must not retain the old anchor: {stamp:?}");
            assert!(!clock.is_valid());
        }
    }

    #[test]
    fn underflow_overflow_and_reset() {
        let mut clock = Timebase::new(500.0);
        clock.observe(&RenderStamp::new(0.0, 100));
        assert_eq!(clock.timestamp_at(-1), None, "host time underflow");
        clock.observe(&RenderStamp::new(0.0, u64::MAX - 10));
        assert_eq!(clock.timestamp_at(1), None, "host time overflow");
        clock.observe(&RenderStamp::new(0.0, 100));
        clock.reset();
        assert_eq!(clock.timestamp_at(0), None, "graph reset");
    }

    #[test]
    fn source_blocks_align_to_the_master_clock() {
        let mut clock = Timebase::new(500.0);
        clock.observe(&RenderStamp::new(1_480_704.0, 1_000_000_000));
        assert_eq!(clock.graph_frame_at(1_000_000_000), Some(1_480_704));
        assert_eq!(clock.graph_frame_at(1_000_256_000), Some(1_481_216), "a look-ahead source block keeps its graph offset");
        assert_eq!(clock.graph_frame_at(999_744_000), Some(1_480_192), "an earlier source block keeps its graph offset");
        assert_eq!(clock.graph_frame_at(0), None, "an invalid source host time is refused");
        assert_eq!(clock.graph_frame_at(u64::MAX), None, "an unbounded source time is refused");
        let mut fractional = Timebase::new(500.25);
        fractional.observe(&RenderStamp::new(0.0, 1_000_000));
        assert_eq!(fractional.timestamp_at(2), Some(1_001_001), "nearest host tick");
        assert_eq!(fractional.ticks_per_frame(), 500.25);
    }
}

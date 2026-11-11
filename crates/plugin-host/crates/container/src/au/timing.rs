//! Latency and tail: from what an Audio Unit reports (seconds, sometimes not finite) to
//! what the protocol carries (frames). Platform independent (tested on Linux).
//!
//! Third-party units report `NaN`, infinities and absurd magnitudes here ("infinite tail"
//! is common, and legitimate). One such value poisons everything that serialises it, so
//! nothing non-finite leaves this module.

/// Latency above this is treated as this (a unit claiming more is wrong, and delay
/// compensation buffers are sized from the value).
pub const MAX_LATENCY_SECONDS: f64 = 60.0;

/// A finite tail above this is reported as infinite.
pub const MAX_FINITE_TAIL_SECONDS: f64 = 24.0 * 3600.0;

/// The protocol's "infinite tail".
pub const TAIL_INFINITE: u64 = u64::MAX;

/// `AUAudioUnit.latency` (seconds) as frames. Non-finite and negative values are 0.
pub fn latency_frames(seconds: f64, sample_rate: f64) -> u32 {
    if !seconds.is_finite() || seconds <= 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0;
    }
    let frames = (seconds.min(MAX_LATENCY_SECONDS) * sample_rate).round();
    if frames >= u32::MAX as f64 {
        u32::MAX
    } else {
        frames as u32
    }
}

/// `AUAudioUnit.tailTime` (seconds) as frames: positive infinity and absurdly long tails
/// are [`TAIL_INFINITE`]; `NaN`, negative infinity and negative values are 0.
pub fn tail_frames(seconds: f64, sample_rate: f64) -> u64 {
    if seconds.is_nan() || seconds <= 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0;
    }
    if seconds > MAX_FINITE_TAIL_SECONDS {
        return TAIL_INFINITE;
    }
    (seconds * sample_rate).round() as u64
}

/// Remembers what the host was last told, so that changes are reported once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reported {
    pub latency: u32,
    pub tail: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Changes {
    pub latency: Option<u32>,
    pub tail: Option<u64>,
}

impl Reported {
    pub fn new(latency_seconds: f64, tail_seconds: f64, sample_rate: f64) -> Reported {
        Reported { latency: latency_frames(latency_seconds, sample_rate), tail: tail_frames(tail_seconds, sample_rate) }
    }

    /// Takes fresh readings; returns what changed (in frames) and remembers it.
    pub fn update(&mut self, latency_seconds: f64, tail_seconds: f64, sample_rate: f64) -> Changes {
        let next = Reported::new(latency_seconds, tail_seconds, sample_rate);
        let changes = Changes {
            latency: (next.latency != self.latency).then_some(next.latency),
            tail: (next.tail != self.tail).then_some(next.tail),
        };
        *self = next;
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_is_rounded_clamped_and_never_garbage() {
        assert_eq!(latency_frames(0.0, 48000.0), 0);
        assert_eq!(latency_frames(64.0 / 48000.0, 48000.0), 64);
        assert_eq!(latency_frames(0.0100000001, 44100.0), 441);
        assert_eq!(latency_frames(1.0, 96000.0), 96000);
        assert_eq!(latency_frames(-0.5, 48000.0), 0);
        assert_eq!(latency_frames(f64::NAN, 48000.0), 0);
        assert_eq!(latency_frames(f64::INFINITY, 48000.0), 0);
        assert_eq!(latency_frames(f64::NEG_INFINITY, 48000.0), 0);
        assert_eq!(latency_frames(1.0e300, 48000.0), 60 * 48000);
        assert_eq!(latency_frames(61.0, 48000.0), 60 * 48000);
        assert_eq!(latency_frames(0.5, f64::NAN), 0);
        assert_eq!(latency_frames(0.5, 0.0), 0);
        assert_eq!(latency_frames(60.0, 1.0e9), u32::MAX);
    }

    #[test]
    fn tail_distinguishes_infinite_from_invalid() {
        assert_eq!(tail_frames(0.0, 48000.0), 0);
        assert_eq!(tail_frames(2.5, 48000.0), 120_000);
        assert_eq!(tail_frames(f64::INFINITY, 48000.0), TAIL_INFINITE);
        assert_eq!(tail_frames(f64::MAX, 48000.0), TAIL_INFINITE);
        assert_eq!(tail_frames(1.0e9, 48000.0), TAIL_INFINITE);
        assert_eq!(tail_frames(f64::NAN, 48000.0), 0);
        assert_eq!(tail_frames(f64::NEG_INFINITY, 48000.0), 0);
        assert_eq!(tail_frames(-1.0, 48000.0), 0);
        assert_eq!(tail_frames(3600.0, 192000.0), 3600 * 192000);
        assert_eq!(tail_frames(1.0, f64::INFINITY), 0);
    }

    #[test]
    fn changes_are_reported_once() {
        let mut reported = Reported::new(0.0, f64::INFINITY, 48000.0);
        assert_eq!(reported, Reported { latency: 0, tail: TAIL_INFINITE });
        assert_eq!(reported.update(0.0, f64::INFINITY, 48000.0), Changes::default());
        // NaN after "nothing" is still nothing.
        assert_eq!(reported.update(f64::NAN, f64::INFINITY, 48000.0), Changes::default());
        assert_eq!(reported.update(0.001, f64::INFINITY, 48000.0), Changes { latency: Some(48), tail: None });
        assert_eq!(reported.update(0.001, 1.0, 48000.0), Changes { latency: None, tail: Some(48000) });
        assert_eq!(reported.update(0.001, 1.0, 48000.0), Changes::default());
        // A new sample rate changes the frame counts.
        assert_eq!(reported.update(0.001, 1.0, 96000.0), Changes { latency: Some(96), tail: Some(96000) });
    }
}

//! Peak meters: one stereo pair per strip, written by the render and read by the interface's
//! status timer.
//!
//! The render folds each block's post-fader peak into a pending maximum (an atomic per channel:
//! no lock, no allocation). The reader takes the pending values and resets them in one atomic
//! exchange, so a transient between two readings is never lost and is shown exactly once; decay
//! happens in the reader ([`MeterDisplay`]), per reading, independent of block size and sample
//! rate. Values above 1 are kept: they show clipping.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// The pending peaks of one render plan's strips.
pub struct MeterBank {
    owners: Vec<String>,
    /// Two per strip (left, right): bit patterns of finite, non-negative `f32`s, which order like
    /// the numbers themselves.
    pending: Box<[AtomicU32]>,
}

impl MeterBank {
    pub fn new(owners: Vec<String>) -> MeterBank {
        let pending = (0..owners.len() * 2).map(|_| AtomicU32::new(0)).collect();
        MeterBank { owners, pending }
    }

    pub fn owners(&self) -> &[String] {
        &self.owners
    }

    /// Render thread: fold a block's peaks into the strip's pending maximum. Non-finite and
    /// non-positive values are ignored.
    #[inline]
    pub fn accumulate(&self, strip: usize, left: f32, right: f32) {
        for (channel, value) in [left, right].into_iter().enumerate() {
            if value.is_finite() && value > 0.0 {
                if let Some(slot) = self.pending.get(strip * 2 + channel) {
                    slot.fetch_max(value.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }

    /// Reader: the strip's peaks since the last call.
    pub fn take(&self, strip: usize) -> (f32, f32) {
        let take = |index: usize| self.pending.get(index).map_or(0.0, |slot| f32::from_bits(slot.swap(0, Ordering::Relaxed)));
        (take(strip * 2), take(strip * 2 + 1))
    }
}

/// The finite peak magnitude of a block; non-finite samples are skipped.
#[inline]
pub fn block_peak(samples: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    let mut chunks = samples.chunks_exact(8);
    for chunk in &mut chunks {
        for (lane, sample) in lanes.iter_mut().zip(chunk) {
            let magnitude = sample.abs();
            // NaN compares false and infinity is excluded: neither is latched.
            if magnitude > *lane && magnitude <= f32::MAX {
                *lane = magnitude;
            }
        }
    }
    let mut peak = lanes.iter().fold(0.0f32, |peak, lane| peak.max(*lane));
    for sample in chunks.remainder() {
        let magnitude = sample.abs();
        if magnitude > peak && magnitude <= f32::MAX {
            peak = magnitude;
        }
    }
    peak
}

/// What the interface shows: peaks by owner, decaying between readings. Reader thread only.
#[derive(Default)]
pub struct MeterDisplay {
    shown: HashMap<String, (f32, f32)>,
}

/// Per-reading decay of a shown peak (20 readings per second: about -22 dB per second).
const DECAY: f32 = 0.88;
/// Below this a decaying peak snaps to zero.
const FLOOR: f32 = 0.00001;

impl MeterDisplay {
    /// One reading: every strip of `bank` gets the larger of its new peak and its decayed old
    /// one. Strips that left the mixer are forgotten.
    pub fn read(&mut self, bank: &MeterBank) {
        let mut next = HashMap::with_capacity(bank.owners().len());
        for (strip, owner) in bank.owners().iter().enumerate() {
            let (left, right) = bank.take(strip);
            let (old_left, old_right) = self.shown.get(owner).copied().unwrap_or((0.0, 0.0));
            let settle = |captured: f32, old: f32| {
                let decayed = old * DECAY;
                captured.max(if decayed < FLOOR { 0.0 } else { decayed })
            };
            next.insert(owner.clone(), (settle(left, old_left), settle(right, old_right)));
        }
        self.shown = next;
    }

    /// The shown `(left, right)` peak of a strip.
    pub fn peak(&self, owner: &str) -> (f32, f32) {
        self.shown.get(owner).copied().unwrap_or((0.0, 0.0))
    }

    pub fn reset(&mut self) {
        self.shown.clear();
    }

    pub fn all(&self) -> &HashMap<String, (f32, f32)> {
        &self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_accumulate_until_read_and_then_decay_per_reading() {
        let bank = MeterBank::new(vec!["track:a".into(), "master".into()]);
        let mut display = MeterDisplay::default();
        bank.accumulate(0, 0.25, 0.5);
        bank.accumulate(0, 0.75, 0.125);
        bank.accumulate(1, 1.5, 0.0);
        display.read(&bank);
        assert_eq!(display.peak("track:a"), (0.75, 0.5), "the largest block peak since the last reading");
        assert_eq!(display.peak("master"), (1.5, 0.0), "values above unity are kept to show clipping");
        display.read(&bank);
        assert_eq!(display.peak("track:a"), (0.75 * 0.88, 0.5 * 0.88), "one reading later the peak has decayed once");
        bank.accumulate(0, 0.7, 0.0);
        display.read(&bank);
        assert_eq!(display.peak("track:a").0, 0.7, "a new peak above the decayed one replaces it");
        for _ in 0..200 {
            display.read(&bank);
        }
        assert_eq!(display.peak("track:a"), (0.0, 0.0), "a decayed peak reaches exactly zero");
        assert_eq!(display.peak("bus:unknown"), (0.0, 0.0));
    }

    #[test]
    fn non_finite_samples_are_never_latched() {
        assert_eq!(block_peak(&[0.1, -0.6, f32::NAN, f32::INFINITY, 0.2, -f32::INFINITY, 0.0, 0.3, 0.4, -0.9]), 0.9);
        assert_eq!(block_peak(&[]), 0.0);
        assert_eq!(block_peak(&[f32::NAN; 20]), 0.0);
        let bank = MeterBank::new(vec!["a".into()]);
        bank.accumulate(0, f32::NAN, f32::INFINITY);
        bank.accumulate(0, -1.0, 0.0);
        bank.accumulate(7, 1.0, 1.0);
        assert_eq!(bank.take(0), (0.0, 0.0));
    }

    #[test]
    fn a_strip_keeps_its_shown_peak_across_a_new_bank_and_leaves_with_its_strip() {
        let mut display = MeterDisplay::default();
        let first = MeterBank::new(vec!["a".into(), "b".into()]);
        first.accumulate(0, 1.0, 1.0);
        first.accumulate(1, 1.0, 1.0);
        display.read(&first);
        let second = MeterBank::new(vec!["a".into()]);
        display.read(&second);
        assert_eq!(display.peak("a"), (0.88, 0.88));
        assert_eq!(display.peak("b"), (0.0, 0.0));
        assert_eq!(display.all().len(), 1);
    }
}

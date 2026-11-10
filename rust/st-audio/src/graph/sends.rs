//! The built-in send effects: the shared delay and reverb returns of `project.delay` and
//! `project.reverb`.
//!
//! This is the DSP of the engine's own send returns (`send_returns` in
//! `tracker_engine::engine::mixer`: a stereo cross-feedback delay with a tone filter, and a
//! pre-delayed eight-comb/four-allpass reverb), moved out of the engine because the host mixes:
//! each is a return of the graph, fed by every track's post-fader signal through that track's
//! `delaySend` / `reverbSend`, wet only, with its `mix` as return level into the master. The
//! arithmetic and its order are the engine's.

use super::spec::SendEffectsSpec;
use tracker_engine::engine::state::{Allpass, Comb};

const COMB_SIZES: [i32; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_SIZES: [i32; 4] = [556, 441, 341, 225];
const SILENCE: f32 = 1e-7;

/// The delay return. Allocated by `new`; `process` never allocates.
pub struct DelayReturn {
    sample_rate: f64,
    left: Box<[f32]>,
    right: Box<[f32]>,
    position: usize,
    tone_l: f32,
    tone_r: f32,
    /// Frames of silent input and output so far; the return is skipped once the line is empty.
    quiet_frames: usize,
}

impl DelayReturn {
    pub fn new(sample_rate: f64) -> DelayReturn {
        let length = (sample_rate * 4.0) as usize + 1;
        DelayReturn {
            sample_rate,
            left: vec![0.0; length].into_boxed_slice(),
            right: vec![0.0; length].into_boxed_slice(),
            position: 0,
            tone_l: 0.0,
            tone_r: 0.0,
            quiet_frames: usize::MAX / 2,
        }
    }

    /// True while silent input gives silent output (nothing is left in the line).
    pub fn is_idle(&self) -> bool {
        self.quiet_frames >= self.left.len()
    }

    /// In place: the send sum in, the wet signal out (before the return level).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], spec: &SendEffectsSpec, bpm: f64) {
        let size = self.left.len();
        let seconds = if spec.delay_beats > 0.0 { spec.delay_beats * 60.0 / bpm.max(1.0) } else { spec.delay_time as f64 };
        let delay_samples = (seconds * self.sample_rate).clamp(1.0, (size - 1) as f64);
        // 1 = open; otherwise 200 Hz .. 20 kHz on a log scale (st_engine_set_send_effects).
        let tone_coefficient = if spec.delay_tone >= 1.0 {
            1.0
        } else {
            (1.0 - (-2.0 * std::f64::consts::PI * 200.0 * 100.0f64.powf(spec.delay_tone as f64) / self.sample_rate).exp()) as f32
        };
        let mut peak = 0.0f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut read = self.position as f64 - delay_samples;
            if read < 0.0 {
                read += size as f64;
                if read >= size as f64 {
                    read = 0.0;
                }
            }
            let ia = (read as usize).min(size - 1);
            let ib = (ia + 1) % size;
            let fraction = (read - ia as f64) as f32;
            let mut wet_l = self.left[ia] + (self.left[ib] - self.left[ia]) * fraction;
            let mut wet_r = self.right[ia] + (self.right[ib] - self.right[ia]) * fraction;
            if tone_coefficient < 1.0 {
                self.tone_l += tone_coefficient * (wet_l - self.tone_l);
                self.tone_r += tone_coefficient * (wet_r - self.tone_r);
                wet_l = self.tone_l;
                wet_r = self.tone_r;
            }
            let position = self.position.min(size - 1);
            self.left[position] = *l + wet_r * spec.delay_feedback;
            self.right[position] = *r + wet_l * spec.delay_feedback;
            self.position = if position + 1 == size { 0 } else { position + 1 };
            peak = peak.max(l.abs()).max(r.abs()).max(wet_l.abs()).max(wet_r.abs());
            *l = wet_l;
            *r = wet_r;
        }
        self.quiet_frames = if peak < SILENCE { self.quiet_frames.saturating_add(left.len()) } else { 0 };
    }

    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.position = 0;
        self.tone_l = 0.0;
        self.tone_r = 0.0;
        self.quiet_frames = usize::MAX / 2;
    }
}

/// The reverb return.
pub struct ReverbReturn {
    sample_rate: f64,
    pre_l: Box<[f32]>,
    pre_r: Box<[f32]>,
    pre_position: usize,
    comb_l: [Comb; 8],
    comb_r: [Comb; 8],
    allpass_l: [Allpass; 4],
    allpass_r: [Allpass; 4],
    quiet_frames: usize,
}

impl ReverbReturn {
    pub fn new(sample_rate: f64) -> ReverbReturn {
        let pre_length = (sample_rate * 0.5) as usize + 1;
        let comb = |size: i32| Comb { data: vec![0.0; (size as f64 * sample_rate / 44100.0) as usize + 1], position: 0, filter: 0.0 };
        let allpass = |size: i32| Allpass { data: vec![0.0; (size as f64 * sample_rate / 44100.0) as usize + 1], position: 0 };
        ReverbReturn {
            sample_rate,
            pre_l: vec![0.0; pre_length].into_boxed_slice(),
            pre_r: vec![0.0; pre_length].into_boxed_slice(),
            pre_position: 0,
            comb_l: COMB_SIZES.map(comb),
            comb_r: COMB_SIZES.map(|size| comb(size + 23)),
            allpass_l: ALLPASS_SIZES.map(allpass),
            allpass_r: ALLPASS_SIZES.map(|size| allpass(size + 23)),
            quiet_frames: usize::MAX / 2,
        }
    }

    /// True once the tail has decayed below audibility with silent input.
    pub fn is_idle(&self) -> bool {
        // The tail is quiet for a full second: combs this quiet only decay further.
        self.quiet_frames >= self.sample_rate as usize
    }

    /// In place: the send sum in, the wet signal out (before the return level).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], spec: &SendEffectsSpec) {
        let feedback = 0.28 + spec.reverb_size * 0.67;
        let length = self.pre_l.len();
        let lag = (length - 1).min((spec.reverb_predelay as f64 * self.sample_rate) as usize);
        let mut peak = 0.0f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (mut rev_l, mut rev_r) = (*l, *r);
            peak = peak.max(rev_l.abs()).max(rev_r.abs());
            if spec.reverb_predelay > 0.0 {
                let position = self.pre_position.min(length - 1);
                self.pre_l[position] = rev_l;
                self.pre_r[position] = rev_r;
                let read = (position + length - lag) % length;
                rev_l = self.pre_l[read];
                rev_r = self.pre_r[read];
                self.pre_position = if position + 1 == length { 0 } else { position + 1 };
            }
            let (in_l, in_r) = ((rev_l + rev_r * 0.2) * 0.075, (rev_r + rev_l * 0.2) * 0.075);
            let (mut wet_l, mut wet_r) = (0.0f32, 0.0f32);
            for c in 0..8 {
                wet_l += self.comb_l[c].process(in_l, feedback, spec.reverb_damping);
                wet_r += self.comb_r[c].process(in_r, feedback, spec.reverb_damping);
            }
            for a in 0..4 {
                wet_l = self.allpass_l[a].process(wet_l);
                wet_r = self.allpass_r[a].process(wet_r);
            }
            peak = peak.max(wet_l.abs()).max(wet_r.abs());
            *l = wet_l;
            *r = wet_r;
        }
        self.quiet_frames = if peak < SILENCE { self.quiet_frames.saturating_add(left.len()) } else { 0 };
    }

    pub fn clear(&mut self) {
        self.pre_l.fill(0.0);
        self.pre_r.fill(0.0);
        self.pre_position = 0;
        for comb in self.comb_l.iter_mut().chain(self.comb_r.iter_mut()) {
            comb.clear();
        }
        for allpass in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            allpass.clear();
        }
        self.quiet_frames = usize::MAX / 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::spec::GraphSpec;

    #[test]
    fn the_delay_repeats_an_impulse_at_its_time_with_cross_feedback() {
        let mut spec = GraphSpec::empty().effects;
        spec.delay_time = 0.01;
        spec.delay_feedback = 0.5;
        let mut delay = DelayReturn::new(48_000.0);
        assert!(delay.is_idle());
        let (mut left, mut right) = (vec![0.0f32; 2000], vec![0.0f32; 2000]);
        left[0] = 1.0;
        delay.process(&mut left, &mut right, &spec, 120.0);
        assert!(!delay.is_idle());
        // 480 frames later the impulse returns on the left; its feedback crosses to the right.
        assert_eq!(left[0], 0.0, "the return is wet only");
        // (0.01 as f32 is a hair under 480 frames: the read interpolates, hence the tolerance.)
        assert!((left[480] - 1.0).abs() < 1e-4 && right[480] == 0.0);
        assert!((right[960] - 0.5).abs() < 1e-4 && left[960].abs() < 1e-4);
        assert!((left[1440] - 0.25).abs() < 1e-4);
        // Tempo sync: a sixteenth at 120 BPM is 0.125 s = 6000 frames.
        spec.delay_beats = 0.25;
        let mut synced = DelayReturn::new(48_000.0);
        let (mut left, mut right) = (vec![0.0f32; 7000], vec![0.0f32; 7000]);
        left[0] = 1.0;
        synced.process(&mut left, &mut right, &spec, 120.0);
        assert!((left[6000] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_reverb_rings_and_then_goes_idle() {
        let spec = GraphSpec::empty().effects;
        let mut reverb = ReverbReturn::new(48_000.0);
        let (mut left, mut right) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        left[0] = 1.0;
        reverb.process(&mut left, &mut right, &spec);
        assert!(left.iter().chain(&right).any(|sample| sample.abs() > 1e-3), "a tail follows the impulse");
        assert!(!reverb.is_idle());
        let mut blocks = 0;
        while !reverb.is_idle() {
            let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
            reverb.process(&mut l, &mut r, &spec);
            blocks += 1;
            assert!(blocks < 2000, "the tail decays");
        }
    }
}

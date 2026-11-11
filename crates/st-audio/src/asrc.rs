//! Asynchronous sample-rate conversion: audio that arrives on one device's clock, delivered on
//! another's.
//!
//! Two audio devices never run at exactly the same rate, even at the same nominal one: their
//! crystals differ by some tens to hundreds of parts per million, and the difference wanders
//! with temperature. Audio that crosses from an input device to an output device therefore has
//! to be resampled by a ratio that nobody knows in advance and that is never a round number.
//! This module has the three parts of that, without any thread or ring in them (the feed that
//! uses them is [`crate::inputs::LineFeed`]):
//!
//! * [`Kernel`]: the interpolation filter, a Kaiser-windowed sinc low-pass tabulated at
//!   [`PHASES`] positions between two input frames and interpolated linearly between
//!   neighbouring rows, so that an output frame can lie anywhere. The design is the one of the
//!   file converter ([`st_samples::resample::SincDesign`]): 100 dB of rejection, flat up to
//!   85 % of the lower of the two Nyquist frequencies (20.4 kHz between two 48 kHz devices,
//!   18.7 kHz from 44.1 kHz), 88 coefficients when the input is not the faster side. Its delay
//!   is half its length: 44 input frames.
//! * [`Converter`]: the read position in the input, as whole frames and a 32-bit fraction,
//!   and the window of input frames around it. Every output frame advances the position by
//!   the ratio; the position is exact integer arithmetic, so over any run the input consumed
//!   is the sum of the ratios, to the frame.
//! * [`RateLoop`]: the estimate of the ratio. It is told, block by block, how far the delay
//!   through the feed is from its target, and answers with the ratio for the block: a
//!   proportional and an integral term behind two smoothing poles. The integral term is the
//!   measured difference of the two clocks; the proportional term brings the delay back to
//!   its target. The loop starts with a bandwidth of 0.3 Hz and narrows to 0.02 Hz within a
//!   few seconds, which is slow enough that callbacks whose timestamps jitter by half a
//!   millisecond move the pitch by some ten parts per million at most (a cent is 578).
//!
//! Nothing here allocates after construction, locks or performs I/O.

use st_samples::resample::SincDesign;
use std::sync::Arc;

/// Rejection of the interpolation filter in dB.
const ATTENUATION_DB: f64 = 100.0;
/// The flat part of the pass band, as a fraction of the lower Nyquist frequency.
const PASSBAND: f64 = 0.85;
/// Table rows per input frame.
pub const PHASES: usize = 1024;
/// One whole frame in the fraction of a position.
const ONE: u64 = 1 << 32;

/// The largest and the smallest ratio of input to output rate a [`Kernel`] is built for.
pub const RATIO_RANGE: std::ops::RangeInclusive<f64> = 1.0 / 16.0..=8.0;

/// The interpolation filter between an input and an output rate. See the module documentation.
pub struct Kernel {
    taps: usize,
    /// `PHASES + 1` rows of `taps` coefficients; row `p` is the output instant `p / PHASES` of
    /// a frame after the window's frame `taps / 2 - 1`.
    table: Vec<f32>,
}

impl Kernel {
    /// The filter for audio at `input_rate` read at about `output_rate`. Control thread.
    pub fn new(input_rate: f64, output_rate: f64) -> Kernel {
        // Everything above the lower Nyquist frequency is rejected: nothing folds back when
        // the output is the slower side, and no image remains when it is the faster one.
        let design = SincDesign::new(0.5 * (output_rate / input_rate).min(1.0), ATTENUATION_DB, PASSBAND);
        let taps = design.taps();
        let mut table = vec![0f32; (PHASES + 1) * taps];
        let mut row = vec![0f64; taps];
        for phase in 0..=PHASES {
            design.row(phase as f64 / PHASES as f64, &mut row);
            for (tap, value) in row.iter().enumerate() {
                table[phase * taps + tap] = *value as f32;
            }
        }
        Kernel { taps, table }
    }

    /// Coefficients per output frame.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Input frames between a frame's arrival in the window and the output instants it is the
    /// centre of: the filter's delay.
    pub fn half(&self) -> usize {
        self.taps / 2
    }

    /// One output frame from the `taps` input frames around it, `fraction` (0 to 1) of a frame
    /// after the window's frame `half - 1`.
    #[inline]
    fn frame(&self, left: &[f32], right: &[f32], fraction: f64) -> (f32, f32) {
        let position = fraction * PHASES as f64;
        let index = (position as usize).min(PHASES - 1);
        let blend = (position - index as f64) as f32;
        let rows = &self.table[index * self.taps..(index + 2) * self.taps];
        let (near, far) = rows.split_at(self.taps);
        let (left_near, left_far) = dot2(left, near, far);
        let (right_near, right_far) = dot2(right, near, far);
        (left_near + (left_far - left_near) * blend, right_near + (right_far - right_near) * blend)
    }
}

/// The inner products of `x` with `a` and with `b`, four independent sums each (which the
/// compiler turns into SIMD). The lengths are equal and a multiple of four.
#[inline]
fn dot2(x: &[f32], a: &[f32], b: &[f32]) -> (f32, f32) {
    let count = x.len().min(a.len()).min(b.len());
    let (x, a, b) = (&x[..count], &a[..count], &b[..count]);
    let (mut a0, mut a1, mut a2, mut a3) = (0f32, 0f32, 0f32, 0f32);
    let (mut b0, mut b1, mut b2, mut b3) = (0f32, 0f32, 0f32, 0f32);
    let mut index = 0;
    while index + 4 <= count {
        a0 += x[index] * a[index];
        a1 += x[index + 1] * a[index + 1];
        a2 += x[index + 2] * a[index + 2];
        a3 += x[index + 3] * a[index + 3];
        b0 += x[index] * b[index];
        b1 += x[index + 1] * b[index + 1];
        b2 += x[index + 2] * b[index + 2];
        b3 += x[index + 3] * b[index + 3];
        index += 4;
    }
    ((a0 + a2) + (a1 + a3), (b0 + b2) + (b1 + b3))
}

/// A ratio (input frames per output frame) as the step of a [`Converter`]: 32 fraction bits.
pub fn step(ratio: f64) -> u64 {
    (ratio.clamp(1.0 / 1024.0, 1024.0) * ONE as f64).round() as u64
}

/// The read position in a stream of stereo input frames and the frames around it.
///
/// Input frames have indices: the count of frames since the stream began (negative indices
/// are silence before it). The owner asks which frames the next output needs
/// ([`Converter::wanted`]), supplies them ([`Converter::input`]) and renders
/// ([`Converter::render`]).
pub struct Converter {
    kernel: Arc<Kernel>,
    /// Input frames `base .. base + len`.
    left: Box<[f32]>,
    right: Box<[f32]>,
    base: i64,
    len: usize,
    /// The input position of the next output frame: whole frames and a fraction of [`ONE`].
    position: i64,
    fraction: u64,
    max_frames: usize,
}

impl Converter {
    /// A converter that renders at most `max_frames` output frames per call at ratios up to
    /// `max_ratio`. Control thread: this is the only allocation.
    pub fn new(kernel: Arc<Kernel>, max_ratio: f64, max_frames: usize) -> Converter {
        let max_frames = max_frames.max(1);
        let capacity = kernel.taps() + (max_frames as f64 * max_ratio).ceil() as usize + 4;
        Converter { left: vec![0.0; capacity].into_boxed_slice(), right: vec![0.0; capacity].into_boxed_slice(), base: 0, len: 0, position: 0, fraction: 0, max_frames, kernel }
    }

    /// Output frames one [`Converter::render`] call may be asked for.
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// The input position of the next output frame.
    pub fn position(&self) -> f64 {
        self.position as f64 + self.fraction as f64 / ONE as f64
    }

    /// Moves the read position. The window is emptied: [`Converter::wanted`] asks for the
    /// frames around the new position again.
    pub fn seek(&mut self, position: f64) {
        let whole = position.floor();
        self.position = whole as i64;
        self.fraction = (((position - whole) * ONE as f64) as u64).min(ONE - 1);
        self.len = 0;
    }

    /// The input frames to supply before `frames` output frames (at most
    /// [`Converter::max_frames`]) can be rendered with `step`: the index of the first one and
    /// how many.
    pub fn wanted(&mut self, frames: usize, step: u64) -> (i64, usize) {
        let half = self.kernel.half() as i64;
        // What lies before the window of the next output frame is done with.
        let first = self.position - half + 1;
        if self.len == 0 {
            self.base = first;
        } else if first > self.base {
            let done = ((first - self.base) as usize).min(self.len);
            self.left.copy_within(done..self.len, 0);
            self.right.copy_within(done..self.len, 0);
            self.len -= done;
            self.base = if self.len == 0 { first } else { self.base + done as i64 };
        }
        let frames = frames.clamp(1, self.max_frames) as u64;
        let last = self.position + ((self.fraction + step * (frames - 1)) >> 32) as i64;
        let have = self.base + self.len as i64;
        let count = (last + half + 1 - have).max(0) as usize;
        (have, count.min(self.left.len() - self.len))
    }

    /// The buffers for the next `count` input frames, as [`Converter::wanted`] asked for them.
    pub fn input(&mut self, count: usize) -> (&mut [f32], &mut [f32]) {
        let count = count.min(self.left.len() - self.len);
        let range = self.len..self.len + count;
        self.len += count;
        (&mut self.left[range.clone()], &mut self.right[range])
    }

    /// Renders `left.len()` output frames, each `step` after the one before. A frame whose
    /// input was not supplied is silence.
    pub fn render(&mut self, step: u64, left: &mut [f32], right: &mut [f32]) {
        let (taps, half) = (self.kernel.taps(), self.kernel.half() as i64);
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            let start = self.position - half + 1 - self.base;
            (*left, *right) = if start >= 0 && start as usize + taps <= self.len {
                let start = start as usize;
                self.kernel.frame(&self.left[start..start + taps], &self.right[start..start + taps], self.fraction as f64 / ONE as f64)
            } else {
                (0.0, 0.0)
            };
            let next = self.fraction + step;
            self.position += (next >> 32) as i64;
            self.fraction = next & (ONE - 1);
        }
    }
}

/// Bandwidth of the loop once it has settled, and when it starts, in Hz.
const SETTLED_HZ: f64 = 0.02;
const STARTING_HZ: f64 = 0.3;
/// Time constant of the change from the starting to the settled bandwidth, in seconds.
const NARROWING_SECONDS: f64 = 2.0;
/// The largest clock difference the loop follows, and the most it bends the ratio in all.
const MAX_DRIFT: f64 = 2.0e-3;
const MAX_CORRECTION: f64 = 5.0e-3;

/// The estimate of the ratio between two clocks. See the module documentation.
pub struct RateLoop {
    /// Input frames per output frame by the nominal rates.
    nominal: f64,
    input_rate: f64,
    /// The measured difference of the clocks, relative (the integral term).
    drift: f64,
    /// The delay error behind one and two smoothing poles, in input frames.
    smoothed: [f64; 2],
    /// Seconds the loop has run.
    age: f64,
}

impl RateLoop {
    pub fn new(input_rate: f64, output_rate: f64) -> RateLoop {
        RateLoop { nominal: input_rate / output_rate, input_rate, drift: 0.0, smoothed: [0.0; 2], age: 0.0 }
    }

    /// The same loop, starting from a difference of the clocks that is known already
    /// (`ppm`, as [`RateLoop::drift_ppm`] of another loop between the same two devices).
    pub fn starting_at(mut self, ppm: f64) -> RateLoop {
        self.drift = (ppm * 1e-6).clamp(-MAX_DRIFT, MAX_DRIFT);
        self
    }

    /// Input frames per output frame by the nominal rates.
    pub fn nominal(&self) -> f64 {
        self.nominal
    }

    /// Seconds the loop has run.
    pub fn age(&self) -> f64 {
        self.age
    }

    /// The ratio for the next `seconds` of output, given that the delay through the feed is
    /// `error` input frames above its target now.
    pub fn update(&mut self, error: f64, seconds: f64) -> f64 {
        self.age += seconds;
        let omega = std::f64::consts::TAU * (SETTLED_HZ + (STARTING_HZ - SETTLED_HZ) * (-self.age / NARROWING_SECONDS).exp());
        // Two poles a decade above the loop's own frequency keep the callbacks' jitter out of
        // the proportional term and cost the loop little phase.
        let smoothing = 1.0 - (-seconds * 10.0 * omega).exp();
        self.smoothed[0] += smoothing * (error - self.smoothed[0]);
        self.smoothed[1] += smoothing * (self.smoothed[0] - self.smoothed[1]);
        // A second-order loop, critically damped: the delay changes by `input_rate` frames per
        // second for a ratio that is off by one.
        self.drift = (self.drift + omega * omega / self.input_rate * self.smoothed[1] * seconds).clamp(-MAX_DRIFT, MAX_DRIFT);
        let correction = (2.0 * omega / self.input_rate * self.smoothed[1] + self.drift).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        self.nominal * (1.0 + correction)
    }

    /// The ratio without a new measurement: the nominal one bent by the measured difference.
    pub fn held(&self) -> f64 {
        self.nominal * (1.0 + self.drift)
    }

    /// The delay was put back on its target by a jump: what the loop remembers of the error
    /// is void, what it learned about the clocks is not.
    pub fn settle(&mut self) {
        self.smoothed = [0.0; 2];
    }

    /// The measured difference of the two clocks in parts per million: positive when the
    /// input's clock is the faster one.
    pub fn drift_ppm(&self) -> f64 {
        self.drift * 1e6
    }

    /// The largest ratio the loop ever answers with.
    pub fn max_ratio(&self) -> f64 {
        self.nominal * (1.0 + MAX_CORRECTION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// Runs `signal` (a function of the input position in frames) through a converter whose
    /// ratio for output frame `n` is `ratio(n)`, in blocks of `block` frames. Returns the
    /// output, the input position of every output frame, and the input frames supplied.
    fn convert(kernel: &Arc<Kernel>, frames: usize, block: usize, ratio: impl Fn(usize) -> f64, signal: impl Fn(f64) -> f64) -> (Vec<f32>, Vec<f64>, i64) {
        let mut converter = Converter::new(kernel.clone(), 8.0, block);
        converter.seek(0.0);
        let (mut output, mut positions) = (vec![0.0f32; frames], Vec::with_capacity(frames));
        let mut scratch = vec![0.0f32; block];
        let mut supplied = 0i64;
        let mut at = 0;
        while at < frames {
            let count = block.min(frames - at);
            let step = step(ratio(at));
            let (from, wanted) = converter.wanted(count, step);
            let (left, right) = converter.input(wanted);
            for index in 0..wanted {
                // Silence before the stream begins, as a feed supplies it.
                let frame = from + index as i64;
                left[index] = if frame < 0 { 0.0 } else { signal(frame as f64) as f32 };
                right[index] = -left[index];
            }
            supplied = from + wanted as i64;
            let start = converter.position();
            for index in 0..count {
                positions.push(start + index as f64 * step as f64 / ONE as f64);
            }
            converter.render(step, &mut output[at..at + count], &mut scratch[..count]);
            // The second channel goes through the same filter.
            for index in 0..count {
                assert_eq!(scratch[index], -output[at + index]);
            }
            at += count;
        }
        (output, positions, supplied)
    }

    /// The level in dB (relative to `amplitude`) of what `output` holds besides
    /// `amplitude * sin(TAU * cycles * position)`.
    fn residue_db(output: &[f32], positions: &[f64], cycles: f64, amplitude: f64, skip: usize) -> f64 {
        let error: f64 = output.iter().zip(positions).skip(skip).map(|(sample, position)| (*sample as f64 - amplitude * (TAU * cycles * position).sin()).powi(2)).sum();
        let mean = error / (output.len() - skip) as f64;
        10.0 * (mean / (amplitude * amplitude / 2.0)).log10()
    }

    /// Amplitude and phase of the sine at `cycles` per output frame in `output`, by least squares.
    fn fit(output: &[f32], cycles: f64) -> (f64, f64) {
        let (mut ss, mut sc, mut cc, mut xs, mut xc) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for (frame, sample) in output.iter().enumerate() {
            let (sin, cos) = (TAU * cycles * frame as f64).sin_cos();
            ss += sin * sin;
            sc += sin * cos;
            cc += cos * cos;
            xs += *sample as f64 * sin;
            xc += *sample as f64 * cos;
        }
        let determinant = ss * cc - sc * sc;
        let (a, b) = ((xs * cc - xc * sc) / determinant, (xc * ss - xs * sc) / determinant);
        ((a * a + b * b).sqrt(), b.atan2(a))
    }

    #[test]
    fn a_sine_keeps_its_frequency_and_level_through_a_fixed_ratio_and_gains_no_more_than_the_filters_floor() {
        // 44.1 kHz read at 48 kHz by a clock that is 137 ppm slow: nothing about the ratio is round.
        let ratio = 44_100.0 / 48_000.0 * (1.0 + 137e-6);
        let kernel = Arc::new(Kernel::new(44_100.0, 48_000.0));
        assert_eq!((kernel.taps(), kernel.half()), (88, 44));
        let frames = 96_000;
        for (hertz, floor) in [(100.0, -100.0), (1_000.0, -100.0), (10_000.0, -95.0), (18_000.0, -90.0)] {
            let cycles = hertz / 44_100.0;
            let (output, positions, _) = convert(&kernel, frames, 480, |_| ratio, |position| 0.5 * (TAU * cycles * position).sin());
            // Against the same sine at the positions the converter read: everything else is
            // distortion and noise.
            let residue = residue_db(&output, &positions, cycles, 0.5, 200);
            // Frequency and level as an analyser would see them, from two halves of the output.
            let half = frames / 2;
            let out_cycles = cycles * ratio;
            let (first, first_phase) = fit(&output[200..half], out_cycles);
            let (second, second_phase) = fit(&output[half..], out_cycles);
            let turn = (second_phase - first_phase - TAU * out_cycles * (half - 200) as f64).rem_euclid(TAU);
            let turn = if turn > std::f64::consts::PI { turn - TAU } else { turn };
            let measured = (out_cycles + turn / TAU / (half - 200) as f64) * 48_000.0;
            let expected = hertz * ratio * 48_000.0 / 44_100.0;
            let level = 20.0 * (first.min(second) / 0.5).log10();
            println!("{hertz} Hz: residue {residue:.1} dB, level {level:+.5} dB, frequency {measured:.6} Hz (expected {expected:.6})");
            assert!(residue < floor, "{hertz} Hz: {residue:.1} dB of distortion and noise");
            assert!(level.abs() < 0.001 && (20.0 * (first.max(second) / 0.5).log10()).abs() < 0.001, "{hertz} Hz: level {level} dB");
            assert!((measured - expected).abs() < 1e-4, "{hertz} Hz: {measured} Hz instead of {expected}");
        }
        // The same clock difference between two 48 kHz devices, and a tone at 20 kHz.
        let kernel = Arc::new(Kernel::new(48_000.0, 48_000.0));
        let cycles = 20_000.0 / 48_000.0;
        let (output, positions, _) = convert(&kernel, frames, 512, |_| 1.0002, |position| 0.5 * (TAU * cycles * position).sin());
        let residue = residue_db(&output, &positions, cycles, 0.5, 200);
        println!("20000 Hz at 48 kHz: residue {residue:.1} dB");
        assert!(residue < -90.0, "{residue}");
    }

    #[test]
    fn a_slower_output_gets_nothing_from_above_its_nyquist_frequency() {
        // 48 kHz read at 44.1 kHz: the filter is longer by the ratio and cuts at 22.05 kHz.
        let ratio = 48_000.0 / 44_100.0 * (1.0 - 211e-6);
        let kernel = Arc::new(Kernel::new(48_000.0, 44_100.0));
        assert_eq!(kernel.taps(), 96);
        let cycles = 1_000.0 / 48_000.0;
        let (output, positions, _) = convert(&kernel, 88_200, 441, |_| ratio, |position| 0.5 * (TAU * cycles * position).sin());
        let residue = residue_db(&output, &positions, cycles, 0.5, 200);
        assert!(residue < -100.0, "{residue}");
        // 23 kHz would fold back to 21.1 kHz: it is gone instead.
        let cycles = 23_000.0 / 48_000.0;
        let (output, _, _) = convert(&kernel, 88_200, 441, |_| ratio, |position| 0.5 * (TAU * cycles * position).sin());
        let peak = output[200..].iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        let level = 20.0 * (peak as f64 / 0.5).log10();
        println!("48 kHz to 44.1 kHz: 1 kHz residue {residue:.1} dB, 23 kHz at {level:.1} dB");
        assert!(level < -95.0, "{level}");
    }

    #[test]
    fn a_drifting_ratio_bends_nothing_and_no_frame_is_lost_or_invented() {
        // A minute between two 48 kHz clocks whose difference wanders between -300 and +300
        // ppm, the ratio changing with every block of 256 frames.
        let kernel = Arc::new(Kernel::new(48_000.0, 48_000.0));
        let frames = 60 * 48_000;
        let ratio = |frame: usize| 1.0 + 300e-6 * (TAU * 0.05 * (frame / 256 * 256) as f64 / 48_000.0).sin();
        let cycles = 1_000.0 / 48_000.0;
        let (output, positions, supplied) = convert(&kernel, frames, 256, ratio, |position| 0.5 * (TAU * cycles * position).sin());
        let residue = residue_db(&output, &positions, cycles, 0.5, 200);
        println!("drifting ratio: residue {residue:.1} dB");
        assert!(residue < -100.0, "{residue}");
        // The input consumed is the sum of the ratios, to the frame: the converter has read up
        // to the window of its last output frame and not one frame further.
        let consumed: f64 = (0..frames).step_by(256).map(|frame| step(ratio(frame)) as f64 * 256.0).sum::<f64>() / ONE as f64;
        let last = positions[frames - 1].floor() as i64;
        assert!((positions[frames - 1] - (consumed - ratio(frames - 1))).abs() < 1e-3, "{} against {consumed}", positions[frames - 1]);
        assert_eq!(supplied, last + kernel.half() as i64 + 1);
    }

    /// Runs a loop against two clocks that differ by `drift` (relative), with timestamps that
    /// jitter by up to `jitter` seconds either way, for `seconds`. Returns the largest delay
    /// error of the whole run, the largest one after the first minute, the largest deviation
    /// of the ratio from the true one after the first minute, and the loop.
    fn settle(drift: f64, jitter: f64, seconds: f64) -> (f64, f64, f64, RateLoop) {
        let mut rate = RateLoop::new(48_000.0, 48_000.0);
        let (block, mut delay, mut time) = (512.0 / 48_000.0, 0.0f64, 0.0f64);
        let (mut worst_start, mut worst_delay, mut worst_ratio) = (0.0f64, 0.0f64, 0.0f64);
        let mut noise = 0x2545_f491_4f6c_dd1du64;
        while time < seconds {
            noise ^= noise << 13;
            noise ^= noise >> 7;
            noise ^= noise << 17;
            let measured = delay + ((noise >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * jitter * 48_000.0;
            let ratio = rate.update(measured, block);
            // The input delivers (1 + drift) frames per output frame; the loop reads `ratio`.
            delay += (1.0 + drift - ratio) * 512.0;
            time += block;
            worst_start = worst_start.max(delay.abs());
            if time > 60.0 {
                worst_delay = worst_delay.max(delay.abs());
                worst_ratio = worst_ratio.max((ratio - (1.0 + drift)).abs());
            }
        }
        (worst_start, worst_delay, worst_ratio, rate)
    }

    #[test]
    fn the_loop_finds_the_difference_of_two_clocks_and_holds_the_delay() {
        for drift in [200e-6, -200e-6, 1_000e-6, 0.0] {
            let (start, delay, ratio, rate) = settle(drift, 0.0, 600.0);
            println!("{:+.0} ppm: the delay moves by {start:.2} frames at most, {delay:.4} after a minute; ratio within {:.4} ppm after a minute, measured {:+.3} ppm", drift * 1e6, ratio * 1e6, rate.drift_ppm());
            // A device 1 000 ppm off is caught before the delay has moved by a millisecond.
            assert!(start < 48.0 * drift.abs().max(200e-6) / 1_000e-6, "{drift}: {start} frames");
            assert!(delay < 0.5 && ratio < 1e-6, "{drift}: {delay} frames, {ratio}");
            assert!((rate.drift_ppm() - drift * 1e6).abs() < 0.01);
        }
        // Timestamps that jitter by half a millisecond either way (24 frames) move the ratio
        // by a few parts per million and the delay by a frame or two.
        let (_, delay, ratio, rate) = settle(200e-6, 0.5e-3, 600.0);
        println!("+200 ppm with 0.5 ms of jitter: delay within {delay:.2} frames, ratio within {:.2} ppm, measured {:+.2} ppm", ratio * 1e6, rate.drift_ppm());
        assert!(delay < 6.0 && ratio < 30e-6, "{delay} frames, {ratio}");
    }
}

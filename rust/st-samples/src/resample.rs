//! Sample-rate conversion with a polyphase Kaiser-windowed sinc filter.
//!
//! Properties the rest of the crate relies on:
//!
//! * **No delay.** Output frame `n` is the band-limited source evaluated at time
//!   `n / output rate`; silence is assumed before the first and after the last input frame.
//!   An impulse at input frame `k` peaks at output frame `k * out / in`.
//! * **Exact length.** `ceil(input frames * out / in)` frames, computed in integers.
//! * **Block independence.** The output depends only on the input samples, never on how they
//!   were split into `process` calls: two conversions of the same audio are bit-identical. That
//!   also lets a converter start in the middle ([`Resampler::skip_to_output`]) and continue an
//!   earlier conversion sample-exactly, which the preview decoder uses.
//!
//! For the usual rates (44.1/48/88.2/96/192 kHz and their relatives) the ratio is a small
//! fraction and one filter phase per output position is tabulated exactly. Unrelated rates use
//! a dense table with linear interpolation between neighbouring phases.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::{Result, SampleError};

/// Filter quality: stop-band rejection against pass-band width and cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResampleQuality {
    /// For auditioning files in the browser: about 80 dB rejection, flat to 85 % of the lower
    /// Nyquist frequency. Roughly a third of the cost of `Standard`.
    Preview,
    /// For samples loaded into the engine: about 110 dB rejection, flat to 90 % of Nyquist.
    Standard,
    /// For exported files: about 140 dB rejection (below the resolution of 24-bit audio), flat
    /// to 91.5 % of Nyquist (20.2 kHz at 44.1 kHz).
    Mastering,
}

impl ResampleQuality {
    /// (stop-band attenuation in dB, pass-band edge as a fraction of the lower Nyquist,
    /// phases of the interpolated table).
    fn design(self) -> (f64, f64, usize) {
        match self {
            ResampleQuality::Preview => (80.0, 0.85, 512),
            ResampleQuality::Standard => (110.0, 0.90, 1024),
            ResampleQuality::Mastering => (140.0, 0.915, 4096),
        }
    }
}

/// Ratios whose reduced numerator is at most this get one exact filter phase per output phase.
const MAX_EXACT_PHASES: u64 = 1024;

/// Sample rates the converter accepts, in Hz.
pub const SAMPLE_RATE_RANGE: std::ops::RangeInclusive<u32> = 1_000..=768_000;

/// Zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    let quarter = x * x / 4.0;
    while term > sum * 1e-17 {
        term *= quarter / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// The filter of one conversion ratio and quality.
struct Kernel {
    /// Output rate / gcd.
    up: u64,
    /// Input rate / gcd.
    down: u64,
    /// Coefficients per phase (a multiple of 8).
    taps: usize,
    /// `taps / 2`: a phase covers input frames `position - half + 1 ..= position + half`.
    half: usize,
    /// Table rows per unit of fractional position.
    phases: usize,
    /// True when `phases == up`, so every output position has its own row.
    exact: bool,
    /// `(phases or phases + 1) * taps` coefficients.
    table: Vec<f32>,
    /// The same coefficients in double precision, for [`ResampleQuality::Mastering`]: summing
    /// hundreds of products in single precision would put the rounding noise above the filter's
    /// own stop-band floor.
    precise: Option<Vec<f64>>,
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Kernel {
    fn new(in_rate: u32, out_rate: u32, quality: ResampleQuality) -> Kernel {
        let divisor = gcd(in_rate as u64, out_rate as u64);
        let (up, down) = (out_rate as u64 / divisor, in_rate as u64 / divisor);
        let (attenuation, passband, dense_phases) = quality.design();
        // All frequencies in cycles per input sample. Everything above the lower of the two
        // Nyquist frequencies is rejected; the transition band ends exactly there.
        let nyquist = 0.5 * (out_rate as f64 / in_rate as f64).min(1.0);
        let transition = (1.0 - passband) * nyquist;
        let cutoff = nyquist - transition / 2.0;
        let needed = ((attenuation - 7.95) / (14.36 * transition)).ceil() as usize + 1;
        let taps = needed.div_ceil(8).max(2) * 8;
        let half = taps / 2;
        let beta = 0.1102 * (attenuation - 8.7);
        let i0_beta = bessel_i0(beta);
        let exact = up <= MAX_EXACT_PHASES;
        let phases = if exact { up as usize } else { dense_phases };
        let rows = if exact { phases } else { phases + 1 };
        let mut table = vec![0f32; rows * taps];
        let mut precise = (quality == ResampleQuality::Mastering).then(|| vec![0f64; rows * taps]);
        let mut row = vec![0f64; taps];
        for phase in 0..rows {
            let fraction = phase as f64 / phases as f64;
            let mut sum = 0.0;
            for (tap, value) in row.iter_mut().enumerate() {
                // Distance of this input frame from the output instant.
                let distance = tap as f64 - (half as f64 - 1.0) - fraction;
                let reach = distance / half as f64;
                let window = if reach.abs() >= 1.0 { 0.0 } else { bessel_i0(beta * (1.0 - reach * reach).sqrt()) / i0_beta };
                let x = 2.0 * cutoff * distance * std::f64::consts::PI;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                *value = 2.0 * cutoff * sinc * window;
                sum += *value;
            }
            // Unity gain at DC for every phase: no ripple on steady signals.
            for (tap, value) in row.iter().enumerate() {
                table[phase * taps + tap] = (value / sum) as f32;
                if let Some(precise) = precise.as_mut() {
                    precise[phase * taps + tap] = value / sum;
                }
            }
        }
        Kernel { up, down, taps, half, phases, exact, table, precise }
    }

    fn shared(in_rate: u32, out_rate: u32, quality: ResampleQuality) -> Arc<Kernel> {
        type Kernels = HashMap<(u32, u32, ResampleQuality), Arc<Kernel>>;
        static CACHE: OnceLock<Mutex<Kernels>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let key = (in_rate, out_rate, quality);
        if let Some(kernel) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return kernel.clone();
        }
        // Built outside the lock: a second thread may build the same table, which is harmless.
        let kernel = Arc::new(Kernel::new(in_rate, out_rate, quality));
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= 24 {
            guard.clear();
        }
        guard.insert(key, kernel.clone());
        kernel
    }

    /// Input position (whole frames) and phase numerator of output frame `n`.
    #[inline]
    fn position(&self, n: u64) -> (u64, u64) {
        let scaled = n as u128 * self.down as u128;
        ((scaled / self.up as u128) as u64, (scaled % self.up as u128) as u64)
    }

    /// One output sample from `window`, the `taps` input frames around the output instant.
    #[inline]
    fn apply(&self, phase: u64, window: &[f32]) -> f32 {
        if self.exact {
            let row = phase as usize * self.taps;
            dot(window, &self.table[row..row + self.taps])
        } else {
            let position = phase as f64 / self.up as f64 * self.phases as f64;
            let index = (position as usize).min(self.phases - 1);
            let blend = (position - index as f64) as f32;
            let row = index * self.taps;
            let a = dot(window, &self.table[row..row + self.taps]);
            let b = dot(window, &self.table[row + self.taps..row + 2 * self.taps]);
            a + (b - a) * blend
        }
    }

    /// The same with double-precision input history and coefficients.
    #[inline]
    fn apply_precise(&self, precise: &[f64], phase: u64, window: &[f64]) -> f32 {
        if self.exact {
            let row = phase as usize * self.taps;
            dot_precise(window, &precise[row..row + self.taps]) as f32
        } else {
            let position = phase as f64 / self.up as f64 * self.phases as f64;
            let index = (position as usize).min(self.phases - 1);
            let row = index * self.taps;
            let a = dot_precise(window, &precise[row..row + self.taps]);
            let b = dot_precise(window, &precise[row + self.taps..row + 2 * self.taps]);
            (a + (b - a) * (position - index as f64)) as f32
        }
    }
}

/// Inner product with eight independent accumulators, which the compiler turns into SIMD.
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0f32; 8];
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        for k in 0..8 {
            lanes[k] += x[k] * y[k];
        }
    }
    ((lanes[0] + lanes[4]) + (lanes[1] + lanes[5])) + ((lanes[2] + lanes[6]) + (lanes[3] + lanes[7]))
}

/// The same in double precision.
#[inline]
fn dot_precise(a: &[f64], b: &[f64]) -> f64 {
    let mut lanes = [0f64; 8];
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        for k in 0..8 {
            lanes[k] += x[k] * y[k];
        }
    }
    ((lanes[0] + lanes[4]) + (lanes[1] + lanes[5])) + ((lanes[2] + lanes[6]) + (lanes[3] + lanes[7]))
}

/// The input a converter still needs, per channel. Mastering keeps it in double precision so
/// that the inner loop is a plain double-precision product sum.
enum History {
    Single(Vec<Vec<f32>>),
    Double(Vec<Vec<f64>>),
}

impl History {
    fn channels(&self) -> usize {
        match self {
            History::Single(channels) => channels.len(),
            History::Double(channels) => channels.len(),
        }
    }

    fn len(&self) -> usize {
        match self {
            History::Single(channels) => channels[0].len(),
            History::Double(channels) => channels[0].len(),
        }
    }

    fn extend<C: AsRef<[f32]>>(&mut self, input: &[C]) {
        match self {
            History::Single(channels) => channels.iter_mut().zip(input).for_each(|(history, block)| history.extend_from_slice(block.as_ref())),
            History::Double(channels) => channels.iter_mut().zip(input).for_each(|(history, block)| history.extend(block.as_ref().iter().map(|&v| v as f64))),
        }
    }

    fn pad(&mut self, frames: usize) {
        match self {
            History::Single(channels) => channels.iter_mut().for_each(|history| history.extend(std::iter::repeat_n(0.0, frames))),
            History::Double(channels) => channels.iter_mut().for_each(|history| history.extend(std::iter::repeat_n(0.0, frames))),
        }
    }

    fn drop_front(&mut self, frames: usize) {
        match self {
            History::Single(channels) => channels.iter_mut().for_each(|history| drop(history.drain(..frames))),
            History::Double(channels) => channels.iter_mut().for_each(|history| drop(history.drain(..frames))),
        }
    }
}

/// `ceil(input_frames * out_rate / in_rate)`: the exact length of a conversion's output.
/// `None` when the result does not fit in 64 bits.
pub fn resampled_frames(input_frames: u64, in_rate: u32, out_rate: u32) -> Option<u64> {
    if in_rate == 0 {
        return None;
    }
    let scaled = input_frames as u128 * out_rate as u128;
    u64::try_from(scaled.div_ceil(in_rate as u128)).ok()
}

/// A streaming sample-rate converter for any number of channels.
///
/// Feed blocks with [`Resampler::process`], then call [`Resampler::finish`] once. Both append
/// to the output vectors. Not for the audio thread: it allocates.
pub struct Resampler {
    kernel: Option<Arc<Kernel>>,
    in_rate: u32,
    out_rate: u32,
    /// Input frames from absolute index `base` on.
    history: History,
    /// Absolute input index of `history[..][0]`; starts negative (silence before the start).
    base: i64,
    received: u64,
    next_output: u64,
    finished: bool,
}

impl Resampler {
    /// A converter from `in_rate` to `out_rate` Hz for `channels` channels. Equal rates give a
    /// converter that copies its input unchanged.
    pub fn new(in_rate: u32, out_rate: u32, channels: usize, quality: ResampleQuality) -> Result<Self> {
        if !SAMPLE_RATE_RANGE.contains(&in_rate) || !SAMPLE_RATE_RANGE.contains(&out_rate) {
            return Err(SampleError::invalid("Sample rates must be between 1 kHz and 768 kHz."));
        }
        if channels == 0 {
            return Err(SampleError::invalid("A sample-rate conversion needs at least one channel."));
        }
        let kernel = (in_rate != out_rate).then(|| Kernel::shared(in_rate, out_rate, quality));
        let half = kernel.as_ref().map_or(0, |k| k.half);
        let history = match kernel.as_ref().and_then(|k| k.precise.as_ref()) {
            Some(_) => History::Double(vec![vec![0.0; half]; channels]),
            None => History::Single(vec![vec![0.0; half]; channels]),
        };
        Ok(Resampler {
            kernel,
            in_rate,
            out_rate,
            history,
            base: -(half as i64),
            received: 0,
            next_output: 0,
            finished: false,
        })
    }

    /// Number of channels.
    pub fn channels(&self) -> usize {
        self.history.channels()
    }

    /// Output frames a complete input of `input_frames` frames produces.
    pub fn output_frames(&self, input_frames: u64) -> u64 {
        resampled_frames(input_frames, self.in_rate, self.out_rate).unwrap_or(u64::MAX)
    }

    /// Input frames each output frame is computed from (0 when the rates are equal).
    pub fn filter_length(&self) -> usize {
        self.kernel.as_ref().map_or(0, |k| k.taps)
    }

    /// Index of the next output frame that will be produced.
    pub fn next_output_frame(&self) -> u64 {
        self.next_output
    }

    /// Makes the converter start its output at `frame` instead of 0: earlier frames are not
    /// computed. The frames it then produces are bit-identical to frames `frame..` of a full
    /// conversion. Call before or between `process` calls; it never moves backwards.
    pub fn skip_to_output(&mut self, frame: u64) {
        self.next_output = self.next_output.max(frame);
    }

    /// Converts a block (one slice per channel, equal lengths) and appends what can already be
    /// computed to `output` (one vector per channel).
    pub fn process<C: AsRef<[f32]>>(&mut self, input: &[C], output: &mut [Vec<f32>]) {
        debug_assert_eq!(input.len(), self.history.channels());
        debug_assert_eq!(output.len(), self.history.channels());
        if self.finished || input.is_empty() {
            return;
        }
        let frames = input[0].as_ref().len();
        let Some(kernel) = self.kernel.clone() else {
            // Equal rates: a copy, honouring a skipped start.
            let first = self.next_output.saturating_sub(self.received).min(frames as u64) as usize;
            for (out, channel) in output.iter_mut().zip(input) {
                out.extend_from_slice(&channel.as_ref()[first..]);
            }
            self.received += frames as u64;
            self.next_output = self.next_output.max(self.received);
            return;
        };
        self.history.extend(input);
        self.received += frames as u64;
        // Output n needs input up to position(n) + half.
        let available = self.received as i64 - kernel.half as i64;
        self.produce(&kernel, output, |position| (position as i64) < available);
    }

    /// Ends the input: appends the remaining output frames. Further calls do nothing.
    pub fn finish(&mut self, output: &mut [Vec<f32>]) {
        if self.finished {
            return;
        }
        self.finished = true;
        let Some(kernel) = self.kernel.clone() else { return };
        self.history.pad(kernel.half);
        let received = self.received;
        // Output instants before the end of the input: n * down / up < received.
        self.produce(&kernel, output, |position| position < received);
    }

    fn produce(&mut self, kernel: &Kernel, output: &mut [Vec<f32>], ready: impl Fn(u64) -> bool) {
        loop {
            let (position, phase) = kernel.position(self.next_output);
            if !ready(position) {
                break;
            }
            let start = position as i64 - kernel.half as i64 + 1 - self.base;
            if start < 0 {
                // Unreachable: history is only dropped behind the next output's window.
                break;
            }
            let start = start as usize;
            match &self.history {
                History::Single(channels) => {
                    for (history, out) in channels.iter().zip(output.iter_mut()) {
                        out.push(kernel.apply(phase, &history[start..start + kernel.taps]));
                    }
                }
                // Double-precision history exists only together with the precise table.
                History::Double(channels) => {
                    let precise = kernel.precise.as_deref().unwrap_or_default();
                    for (history, out) in channels.iter().zip(output.iter_mut()) {
                        out.push(kernel.apply_precise(precise, phase, &history[start..start + kernel.taps]));
                    }
                }
            }
            self.next_output += 1;
        }
        // Forget input no later output needs. Done in large steps to keep the copy cheap.
        let (position, _) = kernel.position(self.next_output);
        let keep_from = position as i64 - kernel.half as i64 + 1;
        let length = self.history.len();
        let droppable = (keep_from - self.base).clamp(0, length as i64) as usize;
        // A skipped start can put the next window beyond everything received: drop it all.
        if droppable >= 8192 || (droppable > 0 && droppable == length) {
            self.history.drop_front(droppable);
            self.base += droppable as i64;
        }
    }
}

/// Converts complete audio in one call. Returns the input unchanged when the rates are equal.
pub fn resample<C: AsRef<[f32]>>(input: &[C], in_rate: u32, out_rate: u32, quality: ResampleQuality) -> Result<Vec<Vec<f32>>> {
    let mut resampler = Resampler::new(in_rate, out_rate, input.len().max(1), quality)?;
    let frames = input.first().map_or(0, |c| c.as_ref().len());
    let expected = resampler.output_frames(frames as u64) as usize;
    let mut output = vec![Vec::with_capacity(expected); input.len()];
    if !input.is_empty() {
        resampler.process(input, &mut output);
        resampler.finish(&mut output);
    }
    Ok(output)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::f64::consts::PI;

    pub(crate) fn sine(frequency: f64, rate: u32, frames: usize, amplitude: f64) -> Vec<f32> {
        (0..frames).map(|i| (amplitude * (2.0 * PI * frequency * i as f64 / rate as f64).sin()) as f32).collect()
    }

    fn db(value: f64) -> f64 {
        20.0 * value.max(1e-30).log10()
    }

    /// Amplitude and residual RMS of `signal` against a sinusoid of known frequency, on the
    /// interior (the edges hold the filter's start and end transients).
    fn tone_fit(signal: &[f32], frequency: f64, rate: u32, margin: usize) -> (f64, f64) {
        let interior = &signal[margin..signal.len() - margin];
        let w = 2.0 * PI * frequency / rate as f64;
        // Least squares for a*sin + b*cos.
        let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, &y) in interior.iter().enumerate() {
            let (s, c) = (w * (i + margin) as f64).sin_cos();
            ss += s * s;
            cc += c * c;
            sc += s * c;
            ys += y as f64 * s;
            yc += y as f64 * c;
        }
        let det = ss * cc - sc * sc;
        let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
        let mut residual = 0.0;
        for (i, &y) in interior.iter().enumerate() {
            let (s, c) = (w * (i + margin) as f64).sin_cos();
            residual += (y as f64 - a * s - b * c).powi(2);
        }
        ((a * a + b * b).sqrt(), (residual / interior.len() as f64).sqrt())
    }

    fn rms(signal: &[f32]) -> f64 {
        (signal.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / signal.len().max(1) as f64).sqrt()
    }

    pub(crate) struct Quality {
        pub ripple_db: f64,
        pub worst_snr_db: f64,
        pub stopband_db: f64,
        pub sweep_alias_db: f64,
    }

    /// Measures one conversion: pass-band ripple and tone purity up to `passband` of the lower
    /// Nyquist, rejection of tones above the output Nyquist, and the alias level of a full-band
    /// sweep (the last two only when converting down).
    pub(crate) fn measure(in_rate: u32, out_rate: u32, quality: ResampleQuality, passband: f64) -> Quality {
        let frames = in_rate as usize / 2;
        let low_nyquist = in_rate.min(out_rate) as f64 / 2.0;
        let margin = 3000;
        let (mut low, mut high, mut worst_snr) = (f64::MAX, f64::MIN, f64::MAX);
        let steps = 12;
        for step in 0..=steps {
            let frequency = 20.0 + (passband * low_nyquist - 20.0) * step as f64 / steps as f64;
            let out = &resample(&[sine(frequency, in_rate, frames, 0.5)], in_rate, out_rate, quality).unwrap()[0];
            let (amplitude, residual) = tone_fit(out, frequency, out_rate, margin);
            low = low.min(db(amplitude / 0.5));
            high = high.max(db(amplitude / 0.5));
            worst_snr = worst_snr.min(db(amplitude / std::f64::consts::SQRT_2) - db(residual));
        }
        // Converting up there is nothing to alias; images show up in the tone SNR above.
        let (mut stopband, mut sweep_alias) = (f64::NAN, f64::NAN);
        if out_rate < in_rate {
            stopband = f64::NEG_INFINITY;
        }
        if out_rate < in_rate {
            let in_nyquist = in_rate as f64 / 2.0;
            for step in 0..=8 {
                let frequency = low_nyquist + (in_nyquist * 0.999 - low_nyquist) * step as f64 / 8.0;
                let out = &resample(&[sine(frequency, in_rate, frames, 0.5)], in_rate, out_rate, quality).unwrap()[0];
                stopband = stopband.max(db(rms(&out[margin..out.len() - margin]) / (0.5 / std::f64::consts::SQRT_2)));
            }
            // A linear sweep over the whole input band. Once its frequency has passed the output
            // Nyquist, anything left in the output is aliasing.
            let seconds = 2.0;
            let total = (in_rate as f64 * seconds) as usize;
            let sweep: Vec<f32> = (0..total)
                .map(|i| {
                    let t = i as f64 / in_rate as f64;
                    (0.9 * (2.0 * PI * (in_nyquist / (2.0 * seconds)) * t * t).sin()) as f32
                })
                .collect();
            let out = &resample(&[sweep], in_rate, out_rate, quality).unwrap()[0];
            // Frequency reaches f at t = f / in_nyquist * seconds; leave the filter's reach as margin.
            let from = ((low_nyquist * 1.01 / in_nyquist * seconds) * out_rate as f64) as usize + margin;
            let peak = out[from..out.len() - margin].iter().fold(0f64, |m, &v| m.max(v.abs() as f64));
            sweep_alias = db(peak / 0.9);
        }
        Quality { ripple_db: high - low, worst_snr_db: worst_snr, stopband_db: stopband, sweep_alias_db: sweep_alias }
    }

    #[test]
    fn mastering_quality_between_delivery_rates() {
        for (from, to) in [(44_100, 48_000), (48_000, 44_100), (48_000, 96_000), (96_000, 48_000), (44_100, 96_000), (96_000, 44_100)] {
            let q = measure(from, to, ResampleQuality::Mastering, 0.9);
            println!(
                "MEASURE resample mastering {from}->{to}: ripple {:.6} dB, worst tone SNR {:.1} dB, stopband {:.1} dB, sweep alias {:.1} dB",
                q.ripple_db, q.worst_snr_db, q.stopband_db, q.sweep_alias_db
            );
            assert!(q.ripple_db < 0.001, "{from}->{to} ripple {}", q.ripple_db);
            assert!(q.worst_snr_db > 120.0, "{from}->{to} SNR {}", q.worst_snr_db);
            if to < from {
                assert!(q.stopband_db < -125.0, "{from}->{to} stopband {}", q.stopband_db);
                assert!(q.sweep_alias_db < -115.0, "{from}->{to} sweep alias {}", q.sweep_alias_db);
            }
        }
    }

    #[test]
    fn standard_and_preview_quality() {
        for (quality, passband, snr, stop) in [(ResampleQuality::Standard, 0.88, 95.0, -100.0), (ResampleQuality::Preview, 0.83, 70.0, -72.0)] {
            for (from, to) in [(44_100, 48_000), (48_000, 44_100), (96_000, 44_100)] {
                let q = measure(from, to, quality, passband);
                println!(
                    "MEASURE resample {quality:?} {from}->{to}: ripple {:.6} dB, worst tone SNR {:.1} dB, stopband {:.1} dB, sweep alias {:.1} dB",
                    q.ripple_db, q.worst_snr_db, q.stopband_db, q.sweep_alias_db
                );
                assert!(q.ripple_db < 0.01, "{quality:?} {from}->{to} ripple {}", q.ripple_db);
                assert!(q.worst_snr_db > snr, "{quality:?} {from}->{to} SNR {}", q.worst_snr_db);
                if to < from {
                    assert!(q.stopband_db < stop, "{quality:?} {from}->{to} stopband {}", q.stopband_db);
                }
            }
        }
    }

    #[test]
    fn unrelated_rates_use_the_interpolated_table() {
        // 44 099 -> 48 000 does not reduce to a small fraction.
        for (quality, snr) in [(ResampleQuality::Mastering, 110.0), (ResampleQuality::Standard, 90.0)] {
            let out = &resample(&[sine(1000.0, 44_099, 22_050, 0.5)], 44_099, 48_000, quality).unwrap()[0];
            assert_eq!(out.len() as u64, resampled_frames(22_050, 44_099, 48_000).unwrap());
            let (amplitude, residual) = tone_fit(out, 1000.0, 48_000, 2000);
            let ratio = db(amplitude / std::f64::consts::SQRT_2) - db(residual);
            println!("MEASURE resample {quality:?} 44099->48000 (interpolated phases): 1 kHz SNR {ratio:.1} dB");
            assert!((amplitude - 0.5).abs() < 1e-5 && ratio > snr, "amplitude {amplitude}, SNR {ratio}");
        }
    }

    #[test]
    fn exact_lengths_and_impulse_alignment() {
        for (from, to) in [(48_000u32, 44_100u32), (48_000, 96_000), (44_100, 48_000), (96_000, 44_100), (48_000, 48_000)] {
            for frames in [1usize, 2, 3, 159, 160, 161, 4095, 4097] {
                let input = vec![0.25f32; frames];
                let out = resample(&[input], from, to, ResampleQuality::Mastering).unwrap();
                assert_eq!(out[0].len() as u64, (frames as u64 * to as u64).div_ceil(from as u64), "{from}->{to} {frames}");
            }
            // 480 input frames are a whole number of output frames at each of these rates.
            let mut impulse = vec![0f32; 9600];
            impulse[4800] = 1.0;
            let out = &resample(&[impulse], from, to, ResampleQuality::Mastering).unwrap()[0];
            let peak = (0..out.len()).max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs())).unwrap();
            assert_eq!(peak as u64, 4800 * to as u64 / from as u64, "{from}->{to} impulse position");
        }
        assert_eq!(resampled_frames(u64::MAX, 48_000, 96_000), None);
        assert_eq!(resampled_frames(u64::MAX, 48_000, 48_000), Some(u64::MAX));
    }

    #[test]
    fn output_does_not_depend_on_block_size_and_can_start_midway() {
        let left = sine(997.0, 48_000, 20_011, 0.8);
        let right: Vec<f32> = (0..20_011).map(|i| ((i * 7919 % 1000) as f32 / 500.0 - 1.0) * 0.3).collect();
        for to in [44_100u32, 96_000, 37_286] {
            let whole = resample(&[&left, &right], 48_000, to, ResampleQuality::Standard).unwrap();
            for block in [1usize, 37, 4096] {
                let mut resampler = Resampler::new(48_000, to, 2, ResampleQuality::Standard).unwrap();
                let mut out = vec![Vec::new(), Vec::new()];
                for start in (0..left.len()).step_by(block) {
                    let end = (start + block).min(left.len());
                    resampler.process(&[&left[start..end], &right[start..end]], &mut out);
                }
                resampler.finish(&mut out);
                assert!(out == whole, "block size {block} changed the {to} Hz output");
            }
            let length = whole[0].len() as u64;
            for skip in [1u64, length / 4, length * 9 / 10, length - 1] {
                let mut resampler = Resampler::new(48_000, to, 2, ResampleQuality::Standard).unwrap();
                resampler.skip_to_output(skip);
                let mut out = vec![Vec::new(), Vec::new()];
                for start in (0..left.len()).step_by(1000) {
                    let end = (start + 1000).min(left.len());
                    resampler.process(&[&left[start..end], &right[start..end]], &mut out);
                }
                resampler.finish(&mut out);
                assert!(out[0] == whole[0][skip as usize..] && out[1] == whole[1][skip as usize..], "skip {skip} at {to} Hz");
            }
        }
    }

    #[test]
    fn equal_rates_copy_and_bad_rates_are_refused() {
        let input = vec![0.1f32, f32::MAX, -0.0, 1.5];
        assert_eq!(resample(&[&input], 48_000, 48_000, ResampleQuality::Preview).unwrap()[0], input);
        let mut resampler = Resampler::new(48_000, 48_000, 1, ResampleQuality::Preview).unwrap();
        resampler.skip_to_output(3);
        let mut out = vec![Vec::new()];
        resampler.process(&[&input[..2]], &mut out);
        resampler.process(&[&input[2..]], &mut out);
        resampler.finish(&mut out);
        assert_eq!(out[0], vec![1.5]);
        assert!(Resampler::new(0, 48_000, 2, ResampleQuality::Standard).is_err());
        assert!(Resampler::new(48_000, 1_000_000, 2, ResampleQuality::Standard).is_err());
        assert!(Resampler::new(48_000, 44_100, 0, ResampleQuality::Standard).is_err());
    }

    #[test]
    fn steady_signals_keep_their_level() {
        // Each phase has unity gain at DC, so a constant stays constant away from the edges.
        let out = &resample(&[vec![0.7f32; 4000]], 44_100, 48_000, ResampleQuality::Preview).unwrap()[0];
        assert!(out[500..out.len() - 500].iter().all(|&v| (v - 0.7).abs() < 2e-6));
    }
}

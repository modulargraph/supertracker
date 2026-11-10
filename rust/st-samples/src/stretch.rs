//! Offline time stretch: waveform-similarity overlap-add (WSOLA).
//!
//! Playback rate inside each grain stays one, so changing the duration does not transpose the
//! source. Left and right share every chosen offset, which preserves their phase relationship.
//! This is a general-purpose time-domain algorithm, not formant correction or transient warping.

use crate::error::{Result, SampleError};
use crate::op::Operation;
use crate::pcm::{DecodedSample, MAX_SAMPLE_FRAMES};

/// Smallest and largest stretch ratio (output duration / input duration).
pub const STRETCH_RATIO_RANGE: std::ops::RangeInclusive<f64> = 0.25..=4.0;

fn ratio_out_of_range() -> SampleError {
    SampleError::invalid("Stretch ratio must be between 0.25 and 4 (25–400% duration).")
}

/// The stretch ratio of a request: an explicit `ratio`, or `source_bpm / target_bpm`
/// (120 BPM played at 90 BPM lasts 120 / 90 times as long).
pub fn stretch_ratio(ratio: Option<f64>, source_bpm: Option<f64>, target_bpm: Option<f64>) -> Result<f64> {
    let value = match (ratio, source_bpm, target_bpm) {
        (Some(explicit), _, _) => explicit,
        (None, Some(source), Some(target)) => {
            if !(source.is_finite() && target.is_finite() && source > 0.0 && target > 0.0) {
                return Err(SampleError::invalid("Source and target tempo must be positive finite numbers."));
            }
            source / target
        }
        _ => return Err(SampleError::invalid("Set a stretch ratio, or both the source and target tempo.")),
    };
    if value.is_finite() && STRETCH_RATIO_RANGE.contains(&value) {
        Ok(value)
    } else {
        Err(ratio_out_of_range())
    }
}

/// Stretches `source` to `ratio` times its duration (0.25...4) without changing its pitch.
/// The result has exactly `round(frames * ratio)` frames at the same sample rate; a ratio of 1
/// returns the audio unchanged.
///
/// CPU-heavy: run it off the interface and audio threads. `operation` is polled once per grain.
pub fn stretch(source: &DecodedSample, ratio: f64, operation: &mut Operation<'_>) -> Result<DecodedSample> {
    if !(ratio.is_finite() && STRETCH_RATIO_RANGE.contains(&ratio)) {
        return Err(ratio_out_of_range());
    }
    let input_count = source.left.len();
    if input_count == 0 || source.right.len() != input_count || !source.sample_rate.is_finite() || source.sample_rate <= 0.0 {
        return Err(SampleError::invalid("The sample has invalid audio data."));
    }
    if !source.left.iter().all(|v| v.is_finite()) || !source.right.iter().all(|v| v.is_finite()) {
        return Err(SampleError::invalid("The sample contains non-finite audio values."));
    }
    let output_length = input_count as f64 * ratio;
    if output_length > MAX_SAMPLE_FRAMES as f64 {
        return Err(SampleError::invalid("The stretched sample exceeds the 100 million frame limit. Split the recording first."));
    }
    if ratio == 1.0 {
        return Ok(source.clone());
    }
    let output_count = (output_length.round() as usize).max(1);
    // Very short material has no stable periodic context. Extending its one available grain
    // is still preferable to resampling and changing its pitch.
    if input_count < 16 {
        let left = (0..output_count).map(|i| source.left[i.min(input_count - 1)]).collect();
        let right = (0..output_count).map(|i| source.right[i.min(input_count - 1)]).collect();
        return Ok(source.with_audio(left, right));
    }

    // About 40 ms balances sustained tones and rhythmic material. The search is half a window,
    // bounded in source time instead of accumulating timing drift.
    let desired_window = ((source.sample_rate * 0.040) as usize).clamp(64, 4096);
    let window = desired_window.min(input_count);
    let hop = (window / 4).max(4);
    let overlap = window - hop;
    let search_radius = (window / 2).max(8);
    let last_input_start = input_count.saturating_sub(window);
    let correlation_stride = (overlap / 128).max(1);
    let coarse_stride = ((source.sample_rate / 12_000.0) as usize).max(1);
    // The half-sample offset gives non-zero endpoint weights for edge normalisation.
    let weight: Vec<f32> = (0..window)
        .map(|index| {
            let s = (std::f64::consts::PI * (index as f64 + 0.5) / window as f64).sin();
            (s * s) as f32
        })
        .collect();
    let mut left = vec![0f32; output_count];
    let mut right = vec![0f32; output_count];
    let mut normalization = vec![0f32; output_count];
    let (sl, sr) = (&source.left, &source.right);
    let mut previous_start = 0usize;
    let mut output_start = 0usize;

    while output_start < output_count {
        operation.check()?;
        let expected = ((output_start as f64 / ratio).round() as usize).min(last_input_start);
        let mut input_start = expected;
        if output_start > 0 && last_input_start > 0 {
            let lower = expected.saturating_sub(search_radius);
            let upper = (expected + search_radius).min(last_input_start);
            let reference_start = (previous_start + hop).min(last_input_start + hop);
            let mut reference_energy = 0f64;
            for n in (0..overlap).step_by(correlation_stride) {
                let (l, r) = (sl[reference_start + n] as f64, sr[reference_start + n] as f64);
                reference_energy += l * l + r * r;
            }
            if reference_energy > 1e-12 {
                // Both channels contribute separately so anti-phase stereo cannot cancel into a
                // silent analysis signal.
                let similarity = |candidate: usize| -> f64 {
                    let (mut dot, mut energy) = (0f64, 0f64);
                    for n in (0..overlap).step_by(correlation_stride) {
                        let (l, r) = (sl[candidate + n] as f64, sr[candidate + n] as f64);
                        dot += l * sl[reference_start + n] as f64 + r * sr[reference_start + n] as f64;
                        energy += l * l + r * r;
                    }
                    if energy <= 1e-12 {
                        return -2.0;
                    }
                    let correlation = dot / (reference_energy * energy).sqrt();
                    // Prefer nearby equally good matches. This avoids time drift on periodic
                    // content and excessive movement of attacks.
                    correlation - 0.002 * candidate.abs_diff(expected) as f64 / search_radius as f64
                };
                let mut best_score = similarity(expected);
                for candidate in (lower..=upper).step_by(coarse_stride) {
                    let score = similarity(candidate);
                    if score > best_score {
                        best_score = score;
                        input_start = candidate;
                    }
                }
                let refine_lower = lower.max(input_start.saturating_sub(coarse_stride));
                let refine_upper = upper.min(input_start + coarse_stride);
                for candidate in refine_lower..=refine_upper {
                    let score = similarity(candidate);
                    if score > best_score {
                        best_score = score;
                        input_start = candidate;
                    }
                }
            }
        }

        let available = window.min(output_count - output_start);
        for n in 0..available {
            let gain = weight[n];
            left[output_start + n] += sl[input_start + n] * gain;
            right[output_start + n] += sr[input_start + n] * gain;
            normalization[output_start + n] += gain;
        }
        previous_start = input_start;
        output_start += hop;
        operation.report(output_start.min(output_count) as u64, Some(output_count as u64));
    }
    for index in 0..output_count {
        let gain = normalization[index].max(1e-12);
        left[index] /= gain;
        right[index] /= gain;
    }
    Ok(source.with_audio(left, right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const RATE: f64 = 48_000.0;

    fn sample(left: Vec<f32>, right: Option<Vec<f32>>, rate: f64) -> DecodedSample {
        let right = right.unwrap_or_else(|| left.clone());
        DecodedSample::new("generated.wav", rate, left, right, 2)
    }

    fn run(source: &DecodedSample, ratio: f64) -> Result<DecodedSample> {
        stretch(source, ratio, &mut Operation::new())
    }

    /// Mean frequency from upward zero crossings, ignoring the edges.
    fn frequency(values: &[f32], rate: f64) -> f64 {
        let trim = (values.len() / 4).min((rate * 0.080) as usize);
        let end = values.len() - trim;
        if end <= trim + 1 {
            return 0.0;
        }
        let crossings: Vec<usize> = (trim + 1..end).filter(|&i| values[i - 1] <= 0.0 && values[i] > 0.0).collect();
        match (crossings.first(), crossings.last()) {
            (Some(&first), Some(&last)) if last > first => (crossings.len() - 1) as f64 * rate / (last - first) as f64,
            _ => 0.0,
        }
    }

    fn tone(hz: f64, amplitude: f64) -> Vec<f32> {
        (0..48_000).map(|i| (amplitude * (2.0 * PI * hz * i as f64 / RATE).sin()) as f32).collect()
    }

    #[test]
    fn sustained_tone_keeps_length_pitch_stereo_and_level() {
        let values = tone(440.0, 0.7);
        let original = sample(values.clone(), Some(values.iter().map(|v| -v).collect()), RATE);
        for ratio in [0.25, 0.5, 0.75, 1.5, 2.0, 4.0] {
            let result = run(&original, ratio).unwrap();
            assert_eq!(result.left.len(), (values.len() as f64 * ratio).round() as usize, "exact output length at ratio {ratio}");
            assert_eq!(result.sample_rate, RATE);
            let hz = frequency(&result.left, RATE);
            assert!((hz - 440.0).abs() < 4.4, "pitch preserved within 1% at ratio {ratio}: {hz} Hz");
            assert!(result.left.iter().zip(&result.right).all(|(l, r)| (l + r).abs() < 0.000_001), "anti-phase stereo preserved");
            assert!(result.left.iter().all(|v| v.is_finite() && v.abs() <= 0.701), "finite bounded audio");
            let middle = &result.left[result.left.len() / 4..result.left.len() / 4 + result.left.len() / 2];
            let rms = (middle.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / middle.len() as f64).sqrt();
            assert!(rms > 0.40, "sustained tone remains audible at ratio {ratio}: rms {rms}");
            println!("MEASURE stretch 440 Hz ratio {ratio}: pitch {hz:.2} Hz, rms {rms:.3}");
        }
        let unchanged = run(&original, 1.0).unwrap();
        assert!(unchanged.left == original.left && unchanged.right == original.right, "unity stretch is lossless");
        assert_eq!(original.left, values, "source PCM remains unchanged");
    }

    #[test]
    fn bass_and_treble_pitch() {
        for fundamental in [55.0, 110.0, 997.0] {
            let source = sample(tone(fundamental, 0.6), None, RATE);
            for ratio in [0.5, 2.0] {
                let hz = frequency(&run(&source, ratio).unwrap().left, RATE);
                assert!((hz - fundamental).abs() / fundamental < 0.01, "pitch retained: {fundamental} Hz -> {hz}, ratio {ratio}");
            }
        }
    }

    #[test]
    fn rhythmic_attacks_stay_at_their_stretched_times() {
        // Wideband decaying drum-like attacks at known times.
        let mut percussion = vec![0f32; 96_000];
        let attacks = [0.20, 0.65, 1.10, 1.55];
        let mut random: u32 = 0x1234_5678;
        for onset in attacks {
            let offset = (onset * RATE) as usize;
            for n in 0..2_880 {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (random >> 8) as f64 / 0x00ff_ffff as f64 * 2.0 - 1.0;
                let envelope = (-(n as f64) / 560.0).exp();
                percussion[offset + n] = (0.8 * envelope * (0.65 * (2.0 * PI * 130.0 * n as f64 / RATE).sin() + 0.35 * noise)) as f32;
            }
        }
        let rms_near = |values: &[f32], time: f64, radius: f64| {
            let lower = ((time - radius) * RATE).max(0.0) as usize;
            let upper = (((time + radius) * RATE) as usize).min(values.len());
            if upper <= lower {
                return 0.0;
            }
            (values[lower..upper].iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / (upper - lower) as f64).sqrt()
        };
        let source = sample(percussion, None, RATE);
        for ratio in [0.5, 1.5, 2.0] {
            let result = run(&source, ratio).unwrap();
            for onset in attacks {
                let attack = rms_near(&result.left, onset * ratio + 0.010, 0.070);
                let gap = rms_near(&result.left, (onset + 0.23) * ratio, 0.040);
                assert!(attack > 0.025, "attack retained near stretched time at {onset}, ratio {ratio}: {attack}");
                assert!(gap < attack * 0.20, "attacks remain distinct, ratio {ratio}: gap {gap}, attack {attack}");
            }
        }
    }

    #[test]
    fn silence_short_input_and_validation() {
        for ratio in [0.25, 4.0] {
            let silence = run(&sample(vec![0.0; 2048], None, RATE), ratio).unwrap();
            assert!(silence.left.iter().all(|&v| v == 0.0), "silent input remains silent");
        }
        let short = run(&sample(vec![0.1, -0.1, 0.2], None, RATE), 2.0).unwrap();
        assert!(short.left.len() == 6 && short.left.iter().all(|v| v.is_finite()));
        assert!((stretch_ratio(None, Some(120.0), Some(90.0)).unwrap() - 4.0 / 3.0).abs() < 1e-9, "tempo conversion direction");
        assert_eq!(stretch_ratio(Some(2.0), Some(120.0), Some(0.0)).unwrap(), 2.0, "an explicit ratio wins");
        let original = sample(tone(440.0, 0.5), None, RATE);
        for invalid in [f64::NAN, f64::INFINITY, 0.0, -1.0, 0.24, 4.01] {
            assert!(run(&original, invalid).is_err(), "ratio {invalid} rejected");
            assert!(stretch_ratio(Some(invalid), None, None).is_err());
        }
        assert!(stretch_ratio(None, Some(120.0), Some(0.0)).is_err(), "zero tempo rejected");
        assert!(stretch_ratio(None, Some(120.0), None).is_err());
        assert!(run(&sample(vec![f32::NAN, 0.0], None, RATE), 2.0).is_err(), "non-finite input rejected");
        assert!(run(&sample(vec![0.0, 1.0], Some(vec![0.0]), RATE), 2.0).is_err(), "mismatched channels rejected");
        assert!(run(&sample(vec![0.0, 1.0], None, 0.0), 2.0).is_err(), "invalid sample rate rejected");
    }

    #[test]
    fn cancellation_and_progress() {
        let source = sample(tone(220.0, 0.5), None, RATE);
        let token = crate::CancelToken::new();
        token.cancel();
        assert!(stretch(&source, 2.0, &mut Operation::new().with_cancel(&token)).unwrap_err().is_cancelled());
        let mut last = 0u64;
        let mut calls = 0;
        let mut callback = |p: crate::Progress| {
            assert!(p.done >= last && p.total == Some(96_000));
            last = p.done;
            calls += 1;
        };
        stretch(&source, 2.0, &mut Operation::new().with_progress(&mut callback)).unwrap();
        assert!(calls > 10 && last == 96_000);
    }
}

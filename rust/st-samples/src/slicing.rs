//! Offline onset analysis for slicing a sample at its transients.
//!
//! Returns marker positions only; the source PCM is never edited. Memory is bounded by
//! 262 144 analysis windows even for the longest sample the decoder accepts.

use serde::{Deserialize, Serialize};

use crate::error::{Result, SampleError};
use crate::op::Operation;
use crate::pcm::{DecodedSample, MAX_SAMPLE_FRAMES};

const MAXIMUM_WINDOWS: usize = 262_144;

/// What `sample.analyzeSlices` asks for. Missing or non-finite numbers take their defaults;
/// out-of-range numbers are clamped.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SliceRequest {
    /// 0 (only the strongest attacks) ... 1 (also soft ones). Default 0.5.
    pub sensitivity: Option<f64>,
    /// Shortest distance between two markers in seconds, 0.005...2. Default 0.04.
    pub minimum_gap_seconds: Option<f64>,
    /// Most slices to return, 1...128. Default 128.
    pub maximum_slices: Option<f64>,
    /// Start of the analysed region, 0...1 of the sample. Default 0.
    pub start: Option<f64>,
    /// End of the analysed region, 0...1 of the sample. Default 1.
    pub end: Option<f64>,
}

/// The answer to `sample.analyzeSlices`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SliceAnalysis {
    /// Always "transient".
    pub method: &'static str,
    /// Slice starts as fractions (0 inclusive ... 1 exclusive) of the analysed region,
    /// ascending; the first is always 0.
    pub slice_points: Vec<f64>,
    /// The same list (the name older interface code reads).
    pub slices: Vec<f64>,
    /// Number of slices.
    pub slice_count: usize,
    /// The sensitivity used.
    pub sensitivity: f64,
    /// The minimum gap used, in seconds.
    pub minimum_gap_seconds: f64,
    /// The slice limit used.
    pub maximum_slices: usize,
    /// Region start as given.
    pub start: f64,
    /// Region end as given.
    pub end: f64,
    /// Length of the analysed region in seconds.
    pub duration: f64,
    /// Length of the analysed region in frames.
    pub analyzed_frames: usize,
    /// Number of analysis windows.
    pub analysis_windows: usize,
    /// Length of one analysis window in seconds (about 1 ms, longer for very long samples).
    pub analysis_window_seconds: f64,
    /// Attacks found before the slice limit was applied.
    pub candidate_count: usize,
    /// True when more attacks were found than `maximum_slices` allows; the strongest were kept.
    pub capped: bool,
}

struct Candidate {
    frame: usize,
    strength: f64,
}

/// Finds the attacks in `sample` (or in the region the request selects).
///
/// The detector follows stereo power in 1 ms windows after a 25 Hz high-pass, together with the
/// power of the sample-to-sample difference (a cheap brightness measure that finds a snare on
/// top of a bass note). A window is an onset when either measure rises well above its average
/// over the previous 40 ms; the marker is then moved back to the foot of the attack.
pub fn analyze_slices(sample: &DecodedSample, request: &SliceRequest, operation: &mut Operation<'_>) -> Result<SliceAnalysis> {
    let total = sample.left.len();
    if !(sample.sample_rate.is_finite() && (1_000.0..=768_000.0).contains(&sample.sample_rate)) || total == 0 || total != sample.right.len() || total > MAX_SAMPLE_FRAMES {
        return Err(SampleError::invalid("Transient analysis requires a valid stereo sample of at most 100 million frames."));
    }
    let rate = sample.sample_rate;
    let number = |value: Option<f64>, fallback: f64, lower: f64, upper: f64| value.filter(|v| v.is_finite()).unwrap_or(fallback).clamp(lower, upper);
    let sensitivity = number(request.sensitivity, 0.5, 0.0, 1.0);
    let gap_seconds = number(request.minimum_gap_seconds, 0.04, 0.005, 2.0);
    let maximum_slices = number(request.maximum_slices, 128.0, 1.0, 128.0).round() as usize;
    let start = number(request.start, 0.0, 0.0, 1.0);
    let end = number(request.end, 1.0, 0.0, 1.0);
    if end <= start {
        return Err(SampleError::invalid("Set the sample end after its start before detecting transients."));
    }
    let first = ((start * total as f64) as usize).min(total - 1);
    let last = ((end * total as f64) as usize).max(first + 1).min(total);
    let count = last - first;
    let hop = ((rate * 0.001).round() as usize).max(1).max(count.div_ceil(MAXIMUM_WINDOWS));
    let windows = count.div_ceil(hop);
    let gap = ((gap_seconds * rate).round() as usize).max(1);
    let mut energy = vec![0f64; windows];
    let mut brightness = vec![0f64; windows];
    let highpass = (-2.0 * std::f64::consts::PI * 25.0 / rate).exp();
    let (mut previous_left, mut previous_right, mut filtered_left, mut filtered_right) = (0f64, 0f64, 0f64, 0f64);
    let (mut peak_energy, mut peak_brightness) = (0f64, 0f64);
    // Stereo power avoids cancelling anti-phase drums. The 25 Hz high-pass keeps a sustained DC
    // offset from counting as musical energy.
    let clean = |value: f32| if value.is_finite() { (value as f64).clamp(-8.0, 8.0) } else { 0.0 };
    for window in 0..windows {
        if window % 4096 == 0 {
            operation.check()?;
            operation.report((window * hop) as u64, Some(count as u64));
        }
        let begin = first + window * hop;
        let finish = (begin + hop).min(last);
        let (mut sum, mut high_sum) = (0f64, 0f64);
        for frame in begin..finish {
            let (left, right) = (clean(sample.left[frame]), clean(sample.right[frame]));
            let next_left = left - previous_left + highpass * filtered_left;
            let next_right = right - previous_right + highpass * filtered_right;
            let (difference_left, difference_right) = (next_left - filtered_left, next_right - filtered_right);
            sum += next_left * next_left + next_right * next_right;
            high_sum += difference_left * difference_left + difference_right * difference_right;
            (previous_left, previous_right, filtered_left, filtered_right) = (left, right, next_left, next_right);
        }
        let divisor = (2 * (finish - begin)) as f64;
        energy[window] = (sum / divisor).sqrt();
        brightness[window] = (high_sum / divisor).sqrt();
        peak_energy = peak_energy.max(energy[window]);
        peak_brightness = peak_brightness.max(brightness[window]);
    }
    let mut markers = vec![0.0];
    let mut candidate_count = 0;
    if peak_energy > 0.000_001 && maximum_slices > 1 && windows > 2 {
        let history = ((0.04 * rate / hop as f64).round() as usize).max(2);
        let amplitude_floor = (peak_energy * (0.045 - sensitivity * 0.04)).max(0.000_001);
        let brightness_floor = (peak_brightness * (0.045 - sensitivity * 0.04)).max(0.000_001);
        let threshold = 2.9 - sensitivity * 1.35;
        let mut novelty = vec![0f64; windows];
        let (mut sum, mut high_sum) = (0f64, 0f64);
        for window in 0..windows {
            let prior = window.min(history).max(1) as f64;
            let ratio = if energy[window] > amplitude_floor { energy[window] / amplitude_floor.max(sum / prior) } else { 0.0 };
            let high_ratio = if brightness[window] > brightness_floor { brightness[window] / brightness_floor.max(high_sum / prior) } else { 0.0 };
            // A bright release or cut edge is not a new attack: require rising energy.
            novelty[window] = if ratio >= 1.1 { ratio.max(high_ratio * 0.9) } else { 0.0 };
            sum += energy[window];
            high_sum += brightness[window];
            if window >= history {
                sum -= energy[window - history];
                high_sum -= brightness[window - history];
            }
        }
        let mut candidates: Vec<Candidate> = Vec::with_capacity((windows / 2).min(count / gap + 1));
        for window in 1..windows - 1 {
            let strength = novelty[window];
            if !(strength >= threshold && strength >= novelty[window - 1] && strength > novelty[window + 1]) {
                continue;
            }
            // Back up to the foot of the attack, within 8 ms. The marker stays inside the
            // region; no zero-crossing snapping moves it past the attack.
            let mut onset = window;
            let lookback = ((0.008 * rate / hop as f64) as usize).max(1);
            let floor = amplitude_floor.max(energy[window] * 0.12);
            while onset > window.saturating_sub(lookback) && energy[onset - 1] > floor && energy[onset - 1] <= energy[onset] * 1.15 {
                onset -= 1;
            }
            let frame = onset * hop;
            if frame < gap || count - frame < hop.max(1) {
                continue;
            }
            let candidate = Candidate { frame, strength };
            match candidates.last_mut() {
                Some(previous) if frame - previous.frame.min(frame) < gap => {
                    if frame >= previous.frame && strength > previous.strength {
                        *previous = candidate;
                    }
                }
                _ => candidates.push(candidate),
            }
        }
        candidate_count = candidates.len();
        if candidates.len() > maximum_slices - 1 {
            candidates.sort_by(|a, b| b.strength.total_cmp(&a.strength).then(a.frame.cmp(&b.frame)));
            candidates.truncate(maximum_slices - 1);
        }
        candidates.sort_by_key(|candidate| candidate.frame);
        markers.extend(candidates.iter().map(|candidate| candidate.frame as f64 / count as f64));
    }
    Ok(SliceAnalysis {
        method: "transient",
        slice_count: markers.len(),
        slices: markers.clone(),
        slice_points: markers,
        sensitivity,
        minimum_gap_seconds: gap_seconds,
        maximum_slices,
        start,
        end,
        duration: count as f64 / rate,
        analyzed_frames: count,
        analysis_windows: windows,
        analysis_window_seconds: hop as f64 / rate,
        candidate_count,
        capped: candidate_count + 1 > maximum_slices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const RATE: f64 = 48_000.0;

    fn sample(values: Vec<f32>, rate: f64, inverted: bool) -> DecodedSample {
        let right = if inverted { values.iter().map(|v| -v).collect() } else { values.clone() };
        DecodedSample::new("/unchanged/recording.wav", rate, values, right, 2)
    }

    fn noise(seed: &mut u32) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed as f64 / u32::MAX as f64 * 2.0 - 1.0
    }

    fn drum(audio: &mut [f32], time: f64, snare: bool) {
        let start = (time * RATE) as usize;
        let mut seed: u32 = 12_345;
        for n in 0..(RATE * 0.15) as usize {
            if start + n >= audio.len() {
                break;
            }
            let t = n as f64 / RATE;
            let tone = (2.0 * PI * if snare { 190.0 } else { 75.0 } * t).sin();
            let value = if snare { noise(&mut seed) * 0.85 + tone * 0.15 } else { tone } * (-t * if snare { 38.0 } else { 30.0 }).exp();
            audio[start + n] += (value * 0.7) as f32;
        }
    }

    fn analyze(source: &DecodedSample, request: &SliceRequest) -> Result<SliceAnalysis> {
        analyze_slices(source, request, &mut Operation::new())
    }

    fn points(source: &DecodedSample, request: &SliceRequest) -> Vec<f64> {
        let result = analyze(source, request).unwrap();
        let points = result.slice_points.clone();
        assert!(points[0] == 0.0 && points.iter().all(|p| p.is_finite() && *p >= 0.0 && *p < 1.0));
        assert!(points.windows(2).all(|pair| pair[0] < pair[1]), "ascending and unique");
        assert_eq!(result.slice_count, points.len());
        assert_eq!(result.slices, points);
        points
    }

    fn drum_loop() -> (Vec<f32>, [f64; 6]) {
        let mut audio = vec![0f32; (RATE * 2.0) as usize];
        let attacks = [0.0, 0.25, 0.5, 0.875, 1.125, 1.5];
        for (index, time) in attacks.iter().enumerate() {
            drum(&mut audio, *time, index % 2 != 0);
        }
        (audio, attacks)
    }

    #[test]
    fn kick_and_snare_attacks_stereo_phase_and_source_preservation() {
        let (audio, attacks) = drum_loop();
        let source = sample(audio.clone(), RATE, false);
        let detected: Vec<f64> = points(&source, &SliceRequest::default()).iter().map(|p| p * source.duration()).collect();
        println!("MEASURE slicing drum loop markers (s): {detected:?} for attacks {attacks:?}");
        assert_eq!(detected.len(), attacks.len());
        for time in attacks {
            assert!(detected.iter().any(|d| (d - time).abs() < 0.012), "missing drum at {time}");
        }
        assert!(source.left == audio && source.path == "/unchanged/recording.wav");
        assert_eq!(points(&sample(audio, RATE, true), &SliceRequest::default()), points(&source, &SliceRequest::default()));
    }

    #[test]
    fn steady_noise_silence_and_drums_over_noise() {
        let mut seed: u32 = 981;
        let mut noisy: Vec<f32> = (0..(RATE * 3.0) as usize).map(|_| (noise(&mut seed) * 0.025) as f32).collect();
        assert_eq!(points(&sample(noisy.clone(), RATE, false), &SliceRequest::default()), vec![0.0]);
        assert_eq!(points(&sample(vec![0.0; 48_000], RATE, false), &SliceRequest::default()), vec![0.0]);
        assert_eq!(points(&sample(vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0], RATE, false), &SliceRequest::default()), vec![0.0]);
        drum(&mut noisy, 0.5, false);
        drum(&mut noisy, 1.25, true);
        let noisy = sample(noisy, RATE, false);
        let hits: Vec<f64> = points(&noisy, &SliceRequest::default()).iter().map(|p| p * 3.0).collect();
        assert!(hits.len() == 3 && hits.iter().any(|h| (h - 0.5).abs() < 0.012) && hits.iter().any(|h| (h - 1.25).abs() < 0.012), "{hits:?}");
        let low = points(&noisy, &SliceRequest { sensitivity: Some(0.0), ..Default::default() });
        let high = points(&noisy, &SliceRequest { sensitivity: Some(1.0), ..Default::default() });
        assert!(high.len() >= low.len());
    }

    #[test]
    fn sustained_synth_note_and_region_relative_markers() {
        let mut synth = vec![0f32; 48_000];
        for frame in 9_600..40_000 {
            let n = (frame - 9_600) as f64;
            synth[frame] = ((n * 2.0 * PI * 220.0 / RATE).sin() * (n / 144.0).min(1.0) * 0.6) as f32;
        }
        let note = points(&sample(synth, RATE, false), &SliceRequest::default());
        assert!(note.len() == 2 && (note[1] - 0.2).abs() < 0.012, "{note:?}");
        let (audio, _) = drum_loop();
        let region = points(&sample(audio, RATE, false), &SliceRequest { start: Some(0.25), end: Some(0.75), ..Default::default() });
        assert!(region.len() == 3 && (region[1] - 0.375).abs() < 0.012 && (region[2] - 0.625).abs() < 0.012, "{region:?}");
    }

    #[test]
    fn slice_limit_minimum_gap_and_bounds() {
        let (audio, _) = drum_loop();
        let source = sample(audio, RATE, false);
        let limited = analyze(&source, &SliceRequest { maximum_slices: Some(3.0), ..Default::default() }).unwrap();
        assert!(limited.slice_count == 3 && limited.capped);
        let spaced: Vec<f64> = points(&source, &SliceRequest { minimum_gap_seconds: Some(0.4), ..Default::default() }).iter().map(|p| p * 2.0).collect();
        assert!(spaced.windows(2).all(|pair| pair[1] - pair[0] >= 0.4 - 1.0 / RATE), "{spaced:?}");
        assert_eq!(points(&source, &SliceRequest { maximum_slices: Some(1.0), ..Default::default() }), vec![0.0]);
        assert!(analyze(&source, &SliceRequest { start: Some(0.8), end: Some(0.2), ..Default::default() }).is_err());
        let sanitized =
            analyze(&source, &SliceRequest { sensitivity: Some(f64::NAN), minimum_gap_seconds: Some(-5.0), maximum_slices: Some(500.0), ..Default::default() }).unwrap();
        assert_eq!((sanitized.sensitivity, sanitized.minimum_gap_seconds, sanitized.maximum_slices), (0.5, 0.005, 128));
        assert!(analyze(&sample(vec![0.0; 10], 100.0, false), &SliceRequest::default()).is_err(), "implausible sample rate");
        assert!(analyze(&sample(vec![], RATE, false), &SliceRequest::default()).is_err());
    }

    #[test]
    fn long_files_use_a_bounded_number_of_windows() {
        let long = sample(vec![0.0; 1_000_000], 1000.0, false);
        let result = analyze(&long, &SliceRequest::default()).unwrap();
        assert!(result.analysis_windows <= 262_144 && result.slice_count == 1);
        assert_eq!(result.analysis_windows, 250_000);
    }

    #[test]
    fn wire_names_and_cancellation() {
        let (audio, _) = drum_loop();
        let source = sample(audio, RATE, false);
        let request: SliceRequest = serde_json::from_str(r#"{"instrument":0,"sensitivity":0.6,"minimumGapSeconds":0.05,"maximumSlices":16,"start":0,"end":1}"#).unwrap();
        assert_eq!((request.minimum_gap_seconds, request.maximum_slices), (Some(0.05), Some(16.0)));
        let json = serde_json::to_value(analyze(&source, &request).unwrap()).unwrap();
        for key in [
            "method", "slicePoints", "slices", "sliceCount", "sensitivity", "minimumGapSeconds", "maximumSlices", "start", "end", "duration", "analyzedFrames",
            "analysisWindows", "analysisWindowSeconds", "candidateCount", "capped",
        ] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
        assert_eq!(json["method"], "transient");
        let token = crate::CancelToken::new();
        token.cancel();
        assert!(analyze_slices(&source, &request, &mut Operation::new().with_cancel(&token)).unwrap_err().is_cancelled());
    }
}

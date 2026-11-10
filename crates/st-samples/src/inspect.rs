//! Sample-editor queries: min/max waveform columns for a zoomed window and zero-crossing
//! snapping of marker positions. Pure arithmetic on a decoded sample; positions are normalised
//! 0...1 over the whole sample.

use serde::{Deserialize, Serialize};

use crate::pcm::DecodedSample;

/// What `sample.inspect` asks for. Missing or non-finite numbers fall back to their defaults.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InspectRequest {
    /// Window start, 0...1 (default 0).
    pub start: Option<f64>,
    /// Window end, 0...1 (default 1).
    pub end: Option<f64>,
    /// Number of columns, 1...4096 (default 1000).
    pub bins: Option<f64>,
    /// Marker positions to snap to zero crossings.
    pub snap: Option<Vec<f64>>,
    /// Search direction for `snap`: -1 backward, 1 forward, 0 nearest (default).
    pub direction: Option<f64>,
    /// Search reach for `snap` in seconds (default 0.02).
    pub window: Option<f64>,
}

/// The answer to `sample.inspect`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    /// The sample's project reference.
    pub path: String,
    /// Length of the whole sample in frames.
    pub frames: usize,
    /// Frames per second.
    pub sample_rate: f64,
    /// Start of the window actually covered (aligned to a frame). Absent for an empty sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// End of the window actually covered. Absent for an empty sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    /// Lowest value of either channel per column.
    pub min: Vec<f32>,
    /// Highest value of either channel per column.
    pub max: Vec<f32>,
    /// The snapped positions, when `snap` was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapped: Option<Vec<f64>>,
}

fn finite_or(value: Option<f64>, fallback: f64) -> f64 {
    value.filter(|v| v.is_finite()).unwrap_or(fallback)
}

impl DecodedSample {
    /// Waveform columns for a window of the sample and optional zero-crossing snapping.
    pub fn inspect(&self, request: &InspectRequest) -> Inspection {
        let count = self.left.len().min(self.right.len());
        let mut result =
            Inspection { path: self.path.clone(), frames: count, sample_rate: self.sample_rate, start: None, end: None, min: Vec::new(), max: Vec::new(), snapped: None };
        if count == 0 {
            return result;
        }
        let start = finite_or(request.start, 0.0).clamp(0.0, 1.0);
        let end = finite_or(request.end, 1.0).min(1.0).max(start);
        let bins = finite_or(request.bins, 1000.0).clamp(1.0, 4096.0) as usize;
        let first = ((start * count as f64 + 1e-6) as usize).min(count - 1);
        let last = ((end * count as f64 - 1e-6).ceil().max(0.0) as usize).min(count).max(first + 1);
        let span = last - first;
        result.min = Vec::with_capacity(bins);
        result.max = Vec::with_capacity(bins);
        for bin in 0..bins {
            let a = first + bin * span / bins;
            let z = (first + (bin + 1) * span / bins).min(last).max(a + 1);
            let (mut low, mut high) = (f32::MAX, f32::MIN);
            for i in a..z {
                low = low.min(self.left[i].min(self.right[i]));
                high = high.max(self.left[i].max(self.right[i]));
            }
            result.min.push(low);
            result.max.push(high);
        }
        result.start = Some(first as f64 / count as f64);
        result.end = Some(last as f64 / count as f64);
        if let Some(positions) = &request.snap {
            let requested = finite_or(request.direction, 0.0);
            let direction = if requested <= -1.0 { -1 } else if requested >= 1.0 { 1 } else { 0 };
            // Clamp in seconds before multiplying or converting: a JSON number can be finite
            // and still exceed the integer range by many orders of magnitude.
            let rate = if self.sample_rate.is_finite() && self.sample_rate > 0.0 { self.sample_rate } else { 1.0 };
            let seconds = finite_or(request.window, 0.02).max(0.0).min(count as f64 / rate);
            let window = (seconds * rate).max(1.0).min(count as f64) as usize;
            result.snapped = Some(positions.iter().map(|&position| self.zero_crossing(position, direction, window)).collect());
        }
        result
    }

    /// The zero crossing (of the summed channels) nearest to `position`, searching at most
    /// `window` frames. `direction` -1/1 searches only backward/forward, 0 picks the closest.
    /// Returns the (clamped) original position when no crossing is in reach.
    pub fn zero_crossing(&self, position: f64, direction: i32, window: usize) -> f64 {
        let count = self.left.len().min(self.right.len());
        let normalized = if position.is_finite() { position.clamp(0.0, 1.0) } else { 0.0 };
        if count <= 1 {
            return normalized;
        }
        let frame = (normalized * count as f64).round();
        let origin = if frame >= (count - 1) as f64 { count - 1 } else { frame as usize };
        let reach = window.min(count).max(1);
        let value = |i: usize| self.left[i] + self.right[i];
        // A crossing lies between frames i-1 and i.
        let crossing = |i: usize| (value(i - 1) < 0.0) != (value(i) < 0.0);
        let (mut forward, mut backward) = (None, None);
        if direction >= 0 && origin + 1 < count {
            for i in origin + 1..=origin + (count - 1 - origin).min(reach) {
                if crossing(i) {
                    forward = Some(if direction > 0 || value(i).abs() <= value(i - 1).abs() { i } else { i - 1 });
                    break;
                }
            }
        }
        if direction <= 0 && origin >= 1 {
            let lowest = (origin - origin.min(reach) + 1).max(1);
            for i in (lowest..=origin).rev() {
                if crossing(i) {
                    backward = Some(if direction < 0 || value(i - 1).abs() <= value(i).abs() { i - 1 } else { i });
                    break;
                }
            }
        }
        let found = match (forward, backward) {
            (Some(f), Some(b)) => Some(if f - origin <= origin.abs_diff(b) { f } else { b }),
            (Some(f), None) => Some(f),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        match found {
            Some(frame) => frame as f64 / count as f64,
            None => normalized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(values: Vec<f32>, rate: f64) -> DecodedSample {
        DecodedSample::new("synthetic", rate, values.clone(), values, 2)
    }

    fn bins(n: f64) -> InspectRequest {
        InspectRequest { bins: Some(n), ..Default::default() }
    }

    const VALUES: [f32; 8] = [-1.0, -0.5, 0.1, 0.8, 0.7, 0.2, -0.1, -0.8];

    #[test]
    fn bounded_bins_directions_windows_and_positions() {
        let sample = sample(VALUES.to_vec(), 1000.0);
        for (input, expected) in [(1e100, 4096), (-1e100, 1), (1.9, 1), (f64::NAN, 1000), (f64::INFINITY, 1000)] {
            let result = sample.inspect(&bins(input));
            assert_eq!((result.min.len(), result.max.len()), (expected, expected), "bins {input}");
        }
        for direction in [-1e100, 0.0, 1e100] {
            let request = InspectRequest { bins: Some(8.0), direction: Some(direction), window: Some(1e100), snap: Some(vec![0.5]), ..Default::default() };
            let snapped = sample.inspect(&request).snapped.unwrap()[0];
            assert_eq!(snapped, if direction < 0.0 { 0.125 } else { 0.75 }, "huge directional searches keep the intended side");
        }
        for window in [usize::MAX, 0, 1, 3] {
            let snapped = sample.zero_crossing(0.5, 0, window);
            assert!(snapped.is_finite() && (0.0..=1.0).contains(&snapped));
        }
        for position in [f64::MAX, -f64::MAX, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, 1.0] {
            let snapped = sample.zero_crossing(position, 0, usize::MAX);
            assert!(snapped.is_finite() && (0.0..=1.0).contains(&snapped), "position {position}");
        }
    }

    #[test]
    fn ordinary_zoom_and_snap_values() {
        let sample = sample(VALUES.to_vec(), 1000.0);
        let regular = sample.inspect(&InspectRequest { bins: Some(4.0), start: Some(0.25), end: Some(0.75), ..Default::default() });
        assert_eq!((regular.start, regular.end), (Some(0.25), Some(0.75)));
        assert_eq!(regular.min, vec![0.1, 0.8, 0.7, 0.2]);
        assert_eq!(sample.zero_crossing(0.5, 1, 3), 0.75);
        assert_eq!(sample.zero_crossing(0.5, -1, 3), 0.125);
        assert_eq!(sample.zero_crossing(0.5, 0, 1), 0.5, "no crossing in range keeps the marker");
        let empty = DecodedSample::new("empty", 48_000.0, vec![], vec![], 2);
        let inspected = empty.inspect(&bins(1e100));
        assert!(inspected.min.is_empty() && inspected.frames == 0);
        let json = serde_json::to_value(&inspected).unwrap();
        assert!(json.get("start").is_none() && json.get("snapped").is_none() && json["sampleRate"] == 48_000.0);
    }

    #[test]
    fn overview_zoom_and_snapping_on_a_tone() {
        // A 100-frame-period sine: crossings every 50 frames, between frames 24|25, 74|75, ...
        let count = 48_000usize;
        let tone: Vec<f32> = (0..count).map(|i| (0.5 * (2.0 * std::f64::consts::PI * (i as f64 + 0.25) / 100.0).sin()) as f32).collect();
        let source = sample(tone.clone(), 48_000.0);
        let overview = source.inspect(&bins(480.0));
        assert_eq!((overview.max.len(), overview.min.len()), (480, 480));
        assert!(overview.max.iter().all(|v| (v - 0.5).abs() < 0.01) && overview.min.iter().all(|v| (v + 0.5).abs() < 0.01));
        let zoom = source.inspect(&InspectRequest { start: Some(0.5), end: Some(0.5 + 50.0 / count as f64), bins: Some(50.0), ..Default::default() });
        assert_eq!(zoom.max, zoom.min, "at one frame per column the envelope is the sample value");
        assert_eq!(zoom.max, &tone[24_000..24_050]);
        assert!((zoom.start.unwrap() - 0.5).abs() < 1e-9);
        let tiny = source.inspect(&InspectRequest { start: Some(0.25), end: Some(0.25), bins: Some(64.0), ..Default::default() });
        assert_eq!(tiny.max.len(), 64, "a zero-width window still answers");

        let crossing = |frame: usize, direction: i32| (source.zero_crossing(frame as f64 / count as f64, direction, 960) * count as f64).round() as usize;
        let is_zero = |frame: usize| tone[frame].abs() < 0.032;
        let near = crossing(10_010, 0);
        assert!(is_zero(near) && near.abs_diff(10_010) <= 26, "nearest crossing {near}");
        let (ahead, behind) = (crossing(10_010, 1), crossing(10_010, -1));
        assert!(ahead > 10_010 && ahead - 10_010 <= 50 && is_zero(ahead), "forward {ahead}");
        assert!(behind < 10_010 && 10_010 - behind <= 50 && is_zero(behind), "backward {behind}");
        assert!(crossing(ahead, 1) > ahead && crossing(behind, -1) < behind, "repeated snapping progresses");
        let flat = sample(vec![0.25; 1000], 48_000.0);
        assert_eq!(flat.zero_crossing(0.5, 0, 100), 0.5);
        let request = InspectRequest { bins: Some(1.0), snap: Some(vec![0.1, 0.2]), direction: Some(0.0), ..Default::default() };
        let snapped = source.inspect(&request).snapped.unwrap();
        assert!(snapped.len() == 2 && snapped.iter().all(|p| is_zero((p * count as f64).round() as usize)));
    }

    #[test]
    fn request_fields_come_from_camel_case_json() {
        let request: InspectRequest = serde_json::from_str(r#"{"start":0.1,"end":0.9,"bins":32,"snap":[0.5],"direction":-1,"window":0.01,"instrument":3}"#).unwrap();
        assert_eq!(request.bins, Some(32.0));
        assert_eq!(request.snap, Some(vec![0.5]));
        assert_eq!(request.direction, Some(-1.0));
    }
}

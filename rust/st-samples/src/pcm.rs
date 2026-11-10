//! Decoded audio in memory and the summary of it the interface shows.

use serde::Serialize;

use crate::error::{Result, SampleError};
use crate::riff::SampleLoop;

/// The longest sample the sampler accepts, in frames. Longer recordings must be split first.
pub const MAX_SAMPLE_FRAMES: usize = 100_000_000;

/// A sample as the sampler and the sample editor use it: stereo 32-bit float at the file's own
/// sample rate. A mono file is present in both channels; of a file with more than two channels
/// the first two are kept.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedSample {
    /// The project's reference to the sample: a file path or a `builtin:` reference.
    pub path: String,
    /// Frames per second of `left`/`right`.
    pub sample_rate: f64,
    /// Left channel.
    pub left: Vec<f32>,
    /// Right channel, the same length as `left`.
    pub right: Vec<f32>,
    /// Channel count of the source file (1 for mono).
    pub original_channels: usize,
    /// Single-cycle size for wavetable playback, detected from file metadata or size.
    /// `None` when unknown.
    pub cycle_frames: Option<usize>,
    /// Loops stored in the file (WAV `smpl` chunk).
    pub loops: Vec<SampleLoop>,
    /// MIDI root note stored in the file (WAV `smpl`/`inst` chunk).
    pub root_note: Option<u8>,
}

/// What `sample.load` returns about a sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleInfo {
    /// The project's reference to the sample.
    pub path: String,
    /// File name without directory and extension.
    pub name: String,
    /// Length in seconds.
    pub duration: f64,
    /// Frames per second.
    pub sample_rate: f64,
    /// Channel count of the source file.
    pub channels: usize,
    /// Length in frames.
    pub frames: usize,
    /// Up to 512 columns of peak magnitude (0...1 for normalised audio) over the whole sample.
    pub peaks: Vec<f32>,
    /// Single-cycle size for wavetable playback; 0 when unknown.
    pub wavetable_frame_size: usize,
    /// Loops stored in the file.
    pub loops: Vec<SampleLoop>,
    /// MIDI root note stored in the file.
    pub root_note: Option<u8>,
}

impl DecodedSample {
    /// A stereo sample from two channels of equal length.
    pub fn new(path: impl Into<String>, sample_rate: f64, left: Vec<f32>, right: Vec<f32>, original_channels: usize) -> Self {
        DecodedSample { path: path.into(), sample_rate, left, right, original_channels, cycle_frames: None, loops: Vec::new(), root_note: None }
    }

    /// Length in frames.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Length in seconds.
    pub fn duration(&self) -> f64 {
        self.left.len() as f64 / self.sample_rate
    }

    /// Other audio under the same reference, sample rate and source channel count. Cycle size,
    /// loops and root note are not carried over: they described the old audio.
    pub fn with_audio(&self, left: Vec<f32>, right: Vec<f32>) -> DecodedSample {
        DecodedSample::new(self.path.clone(), self.sample_rate, left, right, self.original_channels)
    }

    /// File name without directory and extension ("/a/Kick 01.wav" -> "Kick 01").
    pub fn name(&self) -> String {
        let leaf = crate::error::file_name(&self.path);
        match leaf.rfind('.') {
            Some(dot) if dot > 0 => leaf[..dot].to_string(),
            _ => leaf.to_string(),
        }
    }

    /// The overview the interface draws: up to `bins` columns holding the largest magnitude of
    /// either channel.
    pub fn peaks(&self, bins: usize) -> Vec<f32> {
        let count = self.left.len().min(self.right.len());
        let bins = bins.min(count);
        let mut peaks = vec![0f32; bins];
        if bins == 0 {
            return peaks;
        }
        for (bin, peak) in peaks.iter_mut().enumerate() {
            // Frame i belongs to column i * bins / count.
            let from = (bin * count).div_ceil(bins);
            let to = ((bin + 1) * count).div_ceil(bins);
            let mut value = 0f32;
            for i in from..to {
                value = value.max(self.left[i].abs().max(self.right[i].abs()));
            }
            *peak = value;
        }
        peaks
    }

    /// The summary `sample.load` answers with (512 peak columns).
    pub fn info(&self) -> SampleInfo {
        SampleInfo {
            path: self.path.clone(),
            name: self.name(),
            duration: self.duration(),
            sample_rate: self.sample_rate,
            channels: self.original_channels,
            frames: self.left.len(),
            peaks: self.peaks(512),
            wavetable_frame_size: self.cycle_frames.unwrap_or(0),
            loops: self.loops.clone(),
            root_note: self.root_note,
        }
    }

    /// Checks what every in-memory operation relies on: 1...100 million frames, equal channel
    /// lengths, a positive finite sample rate and finite samples.
    pub fn validate(&self) -> Result<()> {
        let count = self.left.len();
        let valid = (1..=MAX_SAMPLE_FRAMES).contains(&count)
            && self.right.len() == count
            && self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.left.iter().all(|v| v.is_finite())
            && self.right.iter().all(|v| v.is_finite());
        if valid {
            Ok(())
        } else {
            Err(SampleError::invalid("The sample has invalid or empty audio data."))
        }
    }
}

/// Audio with any number of channels, one `Vec` per channel, all the same length.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanarPcm {
    /// Frames per second.
    pub sample_rate: u32,
    /// One vector of samples per channel.
    pub channels: Vec<Vec<f32>>,
}

impl PlanarPcm {
    /// Length in frames.
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Length in seconds.
    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_matches_the_wire_shape() {
        let mut sample = DecodedSample::new("/samples/Kick 01.wav", 48_000.0, vec![0.0, 0.5, -1.0, 0.25], vec![0.1, -0.75, 0.2, 0.0], 1);
        sample.cycle_frames = Some(2048);
        let info = sample.info();
        assert_eq!(info.name, "Kick 01");
        assert_eq!(info.peaks, vec![0.1, 0.75, 1.0, 0.25]);
        let json = serde_json::to_value(&info).unwrap();
        for key in ["path", "name", "duration", "sampleRate", "channels", "frames", "peaks", "wavetableFrameSize"] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
        assert_eq!(json["channels"], 1);
        assert_eq!(json["frames"], 4);
        assert_eq!(json["wavetableFrameSize"], 2048);
        assert_eq!(json["duration"], 4.0 / 48_000.0);
    }

    #[test]
    fn peaks_assign_each_frame_to_one_of_at_most_512_columns() {
        let frames = 5000;
        let left: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
        let sample = DecodedSample::new("ramp", 44_100.0, left.clone(), vec![0.0; frames], 2);
        let peaks = sample.peaks(512);
        assert_eq!(peaks.len(), 512);
        // Reference: the direct per-frame assignment.
        let mut expected = vec![0f32; 512];
        for (i, value) in left.iter().enumerate() {
            let bin = (i * 512 / frames).min(511);
            expected[bin] = expected[bin].max(value.abs());
        }
        assert_eq!(peaks, expected);
        assert_eq!(DecodedSample::new("empty", 44_100.0, vec![], vec![], 2).peaks(512), Vec::<f32>::new());
        assert_eq!(DecodedSample::new("short", 44_100.0, vec![0.5; 3], vec![0.0; 3], 2).peaks(512).len(), 3);
    }

    #[test]
    fn names() {
        let name = |path: &str| DecodedSample::new(path, 1.0, vec![], vec![], 2).name();
        assert_eq!(name("builtin:wavetables/Basic/Pulse 12.5.wav"), "Pulse 12.5");
        assert_eq!(name("/a/b/.hidden"), ".hidden");
        assert_eq!(name("plain"), "plain");
    }
}

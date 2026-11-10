//! Wavetable files: ordinary WAV files holding consecutive single-cycle frames.
//!
//! The cycle size travels in a `clm ` chunk ("<!>2048 ...", the common convention), or in the
//! Surge (`srge`) and u-he (`uhWT`) chunks. Files without one fall back to size heuristics.

use std::path::Path;

use crate::error::Result;
use crate::riff;
use crate::wav::{wav_bytes, Dither, WavEncoding, WavSpec};

pub use crate::riff::frame_size_from_clm;

/// The default cycle: 2048 frames.
pub const DEFAULT_FRAME_SIZE: usize = 2048;

/// Cycle sizes the sampler accepts as an explicit setting.
pub const FRAME_SIZE_RANGE: std::ops::RangeInclusive<usize> = riff::FRAME_SIZE_RANGE;

/// Cycle size declared in a WAV file's chunks; `None` when absent or not a WAV file.
pub fn declared_frame_size(path: &Path) -> Option<usize> {
    riff::read_wav_metadata(path)?.declared_frame_size
}

/// The cycle size of a sample of `frames` frames: the declared size when it is valid and the
/// file holds at least one cycle; otherwise a whole number of default-size cycles, or a short
/// power-of-two file that is itself one cycle. `None` leaves the sampler's default in place.
pub fn detect_frame_size(declared: Option<usize>, frames: usize) -> Option<usize> {
    if let Some(declared) = declared {
        if FRAME_SIZE_RANGE.contains(&declared) && frames >= declared {
            return Some(declared);
        }
    }
    if frames == 0 {
        return None;
    }
    if frames % DEFAULT_FRAME_SIZE == 0 {
        return Some(DEFAULT_FRAME_SIZE);
    }
    if frames <= 4096 && frames.is_power_of_two() && frames >= *FRAME_SIZE_RANGE.start() {
        return Some(frames);
    }
    None
}

/// A mono 32-bit float WAV file of `samples`, with a `clm ` cycle-size chunk when `frame_size`
/// is given.
pub fn wavetable_wav_bytes(samples: &[f32], sample_rate: u32, frame_size: Option<usize>) -> Result<Vec<u8>> {
    let spec = WavSpec { sample_rate, channels: 1, encoding: WavEncoding::Float32, dither: Dither::None };
    let chunks: Vec<_> = frame_size.map(riff::clm_chunk).into_iter().collect();
    wav_bytes(spec, &[samples], &chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_size_detection() {
        assert_eq!(detect_frame_size(Some(256), 1024), Some(256));
        assert_eq!(detect_frame_size(Some(256), 255), None, "a declared cycle longer than the file is ignored");
        assert_eq!(detect_frame_size(Some(4), 4096), Some(2048), "an out-of-range declaration falls back to the size rules");
        assert_eq!(detect_frame_size(None, 2048 * 7), Some(2048));
        assert_eq!(detect_frame_size(None, 512), Some(512));
        assert_eq!(detect_frame_size(None, 8), None);
        assert_eq!(detect_frame_size(None, 96_000), None);
        assert_eq!(detect_frame_size(None, 0), None);
        assert_eq!(detect_frame_size(None, 600), None);
    }

    #[test]
    fn written_wavetables_declare_their_cycle() {
        let dir = std::env::temp_dir().join(format!("st-samples-wavetable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("table.wav");
        let samples: Vec<f32> = (0..1024).map(|i| (i as f32 / 1024.0) - 0.5).collect();
        std::fs::write(&path, wavetable_wav_bytes(&samples, 48_000, Some(256)).unwrap()).unwrap();
        assert_eq!(declared_frame_size(&path), Some(256));
        let decoded = crate::decode_file("table", &path, &mut Default::default()).unwrap();
        assert_eq!(decoded.cycle_frames, Some(256));
        assert_eq!(decoded.original_channels, 1);
        assert_eq!(decoded.left, samples);
        assert_eq!(decoded.right, samples);
        std::fs::write(&path, wavetable_wav_bytes(&samples[..600], 48_000, None).unwrap()).unwrap();
        assert_eq!(declared_frame_size(&path), None);
        assert_eq!(crate::decode_file("table", &path, &mut Default::default()).unwrap().cycle_frames, None);
        std::fs::remove_dir_all(&dir).ok();
    }
}

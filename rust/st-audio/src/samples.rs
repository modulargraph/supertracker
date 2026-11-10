//! Where sample PCM comes from: the seam between the audio core and audio files.
//!
//! The core never opens a file. A project names its samples by path (or by a `builtin:`
//! reference); [`crate::project_sync`] asks a [`SampleSource`] for the decoded PCM of each and
//! installs it in the engine. The integration with `st-samples` implements the trait (decoding
//! on its own worker and answering [`SampleLookup::Pending`] until a file is ready, then calling
//! [`crate::AudioEngine::samples_changed`]); [`MemorySamples`] serves tests and generated audio.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Decoded audio: planar stereo float PCM at its own sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplePcm {
    pub left: Vec<f32>,
    /// The same length as `left` (a mono file is given twice).
    pub right: Vec<f32>,
    pub sample_rate: f64,
    /// Single-cycle size for wavetable playback, when known.
    pub cycle_frames: Option<usize>,
}

impl SamplePcm {
    /// Mono PCM, heard on both channels.
    pub fn mono(samples: Vec<f32>, sample_rate: f64) -> SamplePcm {
        SamplePcm { right: samples.clone(), left: samples, sample_rate, cycle_frames: None }
    }

    pub fn frames(&self) -> usize {
        self.left.len().min(self.right.len())
    }
}

/// The answer to "give me this sample".
#[derive(Debug, Clone)]
pub enum SampleLookup {
    /// Decoded and ready to install.
    Ready(Arc<SamplePcm>),
    /// Being decoded: the slot stays silent; the source reports back through
    /// [`crate::AudioEngine::samples_changed`].
    Pending,
    /// Not available, with a message for the user.
    Missing(String),
}

/// Decoded PCM by project reference. Called on the control thread only; must not block for long.
pub trait SampleSource: Send + Sync {
    fn sample(&self, path: &str) -> SampleLookup;
}

/// A source without any sample: every instrument stays silent.
pub struct NoSamples;

impl SampleSource for NoSamples {
    fn sample(&self, path: &str) -> SampleLookup {
        SampleLookup::Missing(format!("No sample source is attached, so {path} cannot be loaded."))
    }
}

/// Samples held in memory, by path.
#[derive(Default)]
pub struct MemorySamples {
    samples: Mutex<HashMap<String, Arc<SamplePcm>>>,
}

impl MemorySamples {
    pub fn new() -> MemorySamples {
        MemorySamples::default()
    }

    /// Adds or replaces the sample at `path`.
    pub fn insert(&self, path: &str, sample: SamplePcm) {
        self.samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(path.to_string(), Arc::new(sample));
    }

    /// Reads a WAV file with [`read_wav`] and keeps it under its path.
    pub fn insert_wav_file(&self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{} could not be read: {error}", path.display()))?;
        self.insert(&path.to_string_lossy(), read_wav(&bytes)?);
        Ok(())
    }
}

impl SampleSource for MemorySamples {
    fn sample(&self, path: &str) -> SampleLookup {
        match self.samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(path) {
            Some(sample) => SampleLookup::Ready(sample.clone()),
            None => SampleLookup::Missing(format!("{path} is not loaded.")),
        }
    }
}

/// A minimal RIFF/WAVE reader for tests: PCM 8/16/24/32-bit and 32-bit float, any channel count
/// (the first two channels are used). Real files go through `st-samples`.
pub fn read_wav(bytes: &[u8]) -> Result<SamplePcm, String> {
    let bad = |what: &str| format!("Not a WAV file this reader understands ({what}).");
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("no RIFF/WAVE header"));
    }
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (mut format, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32_at(at + 4).ok_or_else(|| bad("truncated chunk"))? as usize;
        let body = at + 8;
        let end = body.checked_add(size).filter(|end| *end <= bytes.len()).unwrap_or(bytes.len());
        match &bytes[at..at + 4] {
            b"fmt " => {
                let tag = u16_at(body).ok_or_else(|| bad("truncated format"))?;
                let channels = u16_at(body + 2).ok_or_else(|| bad("truncated format"))? as usize;
                let rate = u32_at(body + 4).ok_or_else(|| bad("truncated format"))?;
                let bits = u16_at(body + 14).ok_or_else(|| bad("truncated format"))? as usize;
                // WAVE_FORMAT_EXTENSIBLE carries the real tag in the first two bytes of its GUID.
                let tag = if tag == 0xfffe { u16_at(body + 24).unwrap_or(tag) } else { tag };
                format = Some((tag, channels, rate, bits));
            }
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        at = end + (size & 1);
    }
    let (tag, channels, rate, bits) = format.ok_or_else(|| bad("no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    let width = bits / 8;
    if channels == 0 || rate == 0 || !matches!((tag, bits), (1, 8 | 16 | 24 | 32) | (3, 32)) {
        return Err(bad("unsupported sample format"));
    }
    let value = |sample: &[u8]| -> f32 {
        match (tag, bits) {
            (1, 8) => (sample[0] as f32 - 128.0) / 128.0,
            (1, 16) => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0,
            (1, 24) => (i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8) as f32 / 8_388_608.0,
            (1, 32) => i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32 / 2_147_483_648.0,
            _ => f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
        }
    };
    let frame = width * channels;
    let (mut left, mut right) = (Vec::with_capacity(data.len() / frame), Vec::with_capacity(data.len() / frame));
    for chunk in data.chunks_exact(frame) {
        left.push(value(&chunk[..width]));
        right.push(if channels > 1 { value(&chunk[width..2 * width]) } else { value(&chunk[..width]) });
    }
    Ok(SamplePcm { left, right, sample_rate: rate as f64, cycle_frames: None })
}

/// The bytes of a 16-bit PCM WAV file (tests write fixtures with it).
pub fn wav_bytes(left: &[f32], right: Option<&[f32]>, sample_rate: u32) -> Vec<u8> {
    let channels: u16 = if right.is_some() { 2 } else { 1 };
    let data_size = (left.len() * channels as usize * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_size as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_size).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());
    let quantize = |v: f32| ((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes();
    for (index, sample) in left.iter().enumerate() {
        out.extend_from_slice(&quantize(*sample));
        if let Some(right) = right {
            out.extend_from_slice(&quantize(right.get(index).copied().unwrap_or(0.0)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_file_round_trips_through_the_reader() {
        let left: Vec<f32> = (0..64).map(|i| (i as f32 / 64.0) - 0.5).collect();
        let right: Vec<f32> = left.iter().map(|v| -v).collect();
        let pcm = read_wav(&wav_bytes(&left, Some(&right), 44_100)).unwrap();
        assert_eq!((pcm.sample_rate, pcm.frames()), (44_100.0, 64));
        for i in 0..64 {
            assert!((pcm.left[i] - left[i]).abs() < 1.0 / 32000.0 && (pcm.right[i] + pcm.left[i]).abs() < 1.0 / 16000.0);
        }
        let mono = read_wav(&wav_bytes(&left, None, 48_000)).unwrap();
        assert_eq!(mono.left, mono.right);
        assert!(read_wav(b"RIFF....WAVE").is_err() && read_wav(b"nope").is_err());
    }

    #[test]
    fn memory_samples_answer_by_path() {
        let samples = MemorySamples::new();
        samples.insert("/a.wav", SamplePcm::mono(vec![0.5; 8], 48_000.0));
        assert!(matches!(samples.sample("/a.wav"), SampleLookup::Ready(pcm) if pcm.frames() == 8));
        assert!(matches!(samples.sample("/b.wav"), SampleLookup::Missing(_)));
    }
}

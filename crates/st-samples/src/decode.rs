//! Decoding audio files to 32-bit float PCM.
//!
//! Containers and codecs come from the pure-Rust `symphonia` crate: WAV (integer, float,
//! A-law/µ-law, ADPCM), AIFF/AIFC, CAF, FLAC, MP3 (and MPEG layers 1 and 2), AAC-LC in MP4/M4A,
//! ADTS or CAF, ALAC in MP4/M4A or CAF, and Ogg Vorbis. This module adds what a sampler needs on
//! top: encoder delay and padding of AAC in MP4 are removed (so a transient sits where it did
//! before encoding), mono is duplicated to stereo, and WAV sampler chunks (loops, root note,
//! wavetable cycle size) are attached to the result.
//!
//! [`AudioFileReader`] streams blocks; [`decode_file`] and [`decode_planar`] read whole files.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::error::{file_name, Result, SampleError};
use crate::op::Operation;
use crate::pcm::{DecodedSample, PlanarPcm, MAX_SAMPLE_FRAMES};
use crate::resample::{ResampleQuality, Resampler};
use crate::{mp4, riff, wavetable};

/// File extensions (lower case, without dot) of the formats [`AudioFileReader`] can open.
/// Useful for file dialogs and library scans; decoding itself goes by content, not by name.
pub const SUPPORTED_EXTENSIONS: &[&str] =
    &["wav", "wave", "aif", "aiff", "aifc", "caf", "flac", "mp3", "mp2", "mp1", "m4a", "mp4", "m4b", "aac", "ogg", "oga"];

/// What a file says about itself before any audio is decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSpec {
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in the file.
    pub channels: usize,
    /// Playable frames, when the container states them. Encoder delay and padding are excluded
    /// wherever the file declares them. Exact for PCM and for well-formed lossless and MP3/MP4/
    /// Ogg files; a container that only counts whole packets can state a few thousand frames
    /// more than decoding yields, so treat it as an upper bound when sizing buffers.
    pub frames: Option<u64>,
    /// Short container name, e.g. "wave", "aiff", "caf", "isomp4", "ogg", "flac", "mp3".
    pub container: String,
    /// Short codec name, e.g. "pcm_s16le", "flac", "mp3", "aac", "alac", "vorbis".
    pub codec: String,
    /// Bits per sample of the encoded audio, for lossless and PCM sources.
    pub bits_per_sample: Option<u32>,
}

impl SourceSpec {
    /// Length in seconds, when the frame count is known.
    pub fn duration(&self) -> Option<f64> {
        self.frames.map(|frames| frames as f64 / self.sample_rate.max(1) as f64)
    }
}

/// A file being decoded block by block. Blocks are planar 32-bit float at the file's own sample
/// rate and channel count.
pub struct AudioFileReader {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    spec: SourceSpec,
    name: String,
    /// Leading frames still to drop (encoder delay the decoder does not remove itself).
    skip: u64,
    /// Frames still to deliver when the playable length is known exactly.
    remaining: Option<u64>,
    position: u64,
    block: Vec<Vec<f32>>,
    decoded_any: bool,
    finished: bool,
}

fn unsupported(name: &str) -> SampleError {
    SampleError::Unsupported(format!(
        "Cannot read “{name}”: it is not a supported audio file (WAV, AIFF, CAF, FLAC, MP3, AAC/M4A, Apple Lossless or Ogg Vorbis)."
    ))
}

impl AudioFileReader {
    /// Opens `path` and identifies its format from the content (the extension is only a hint).
    pub fn open(path: &Path) -> Result<Self> {
        let shown = path.to_string_lossy();
        let name = file_name(&shown).to_string();
        let file = File::open(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                SampleError::NotFound(format!("Sample file not found: {name}. Use Relink missing project media to locate it."))
            }
            _ => SampleError::Io(format!("Cannot open “{name}”: {error}.")),
        })?;
        if file.metadata().map(|m| m.is_dir()).unwrap_or(false) {
            return Err(unsupported(&name));
        }
        let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        let mut hint = Hint::new();
        if let Some(extension) = &extension {
            hint.with_extension(extension);
        }
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let format = symphonia::default::get_probe()
            .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
            .map_err(|_| unsupported(&name))?;
        let track = format.default_track(TrackType::Audio).ok_or_else(|| unsupported(&name))?;
        let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| unsupported(&name))?;
        let registry = symphonia::default::get_codecs();
        let decoder = registry.make_audio_decoder(params, &AudioDecoderOptions::default()).map_err(|_| {
            SampleError::Unsupported(format!("Cannot read “{name}”: its audio encoding is not supported."))
        })?;
        let codec = registry.get_audio_decoder(params.codec).map(|d| d.codec.info.short_name).unwrap_or("unknown").to_string();
        let container = format.format_info().short_name.to_string();
        let sample_rate = params.sample_rate.filter(|&rate| rate > 0).ok_or_else(|| unsupported(&name))?;
        let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(0);
        let mut frames = track.num_frames;
        let (mut skip, mut remaining) = (0u64, None);
        // The MP4 demuxer reports the encoded length; the edit list holds what should be played.
        if container == "isomp4" {
            if let Some(edit) = mp4::read_edit(path, sample_rate) {
                skip = edit.start;
                let available = frames.map(|total| total.saturating_sub(edit.start));
                let playable = match (edit.frames, available) {
                    (Some(declared), Some(available)) => Some(declared.min(available)),
                    (declared, available) => declared.or(available),
                };
                remaining = playable;
                frames = playable.or(frames);
            }
        }
        let spec = SourceSpec { sample_rate, channels, frames, container, codec, bits_per_sample: params.bits_per_sample };
        let track_id = track.id;
        Ok(AudioFileReader { format, decoder, track_id, spec, name, skip, remaining, position: 0, block: Vec::new(), decoded_any: false, finished: false })
    }

    /// The file's format. The channel count is corrected after the first block for the rare
    /// file whose header does not state it.
    pub fn spec(&self) -> &SourceSpec {
        &self.spec
    }

    /// Frames delivered so far.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Decodes the next block: one slice per channel, all the same non-zero length.
    /// `Ok(None)` at the end of the file.
    ///
    /// A damaged packet in the middle of a file is skipped; a file in which nothing decodes is
    /// an error.
    pub fn next_block(&mut self) -> Result<Option<&[Vec<f32>]>> {
        loop {
            if self.finished || self.remaining == Some(0) {
                self.finished = true;
                return if self.decoded_any { Ok(None) } else { Err(self.unreadable("it contains no audio")) };
            }
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(SymphoniaError::ResetRequired) => {
                    self.finished = true;
                    continue;
                }
                Err(SymphoniaError::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof && self.decoded_any => {
                    // A truncated file: keep what was there.
                    self.finished = true;
                    continue;
                }
                Err(error) => return Err(self.unreadable(&error.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
                Err(error) => {
                    let message = error.to_string();
                    return Err(self.unreadable(&message));
                }
            };
            if decoded.frames() == 0 {
                continue;
            }
            decoded.copy_to_vecs_planar::<f32>(&mut self.block);
            self.spec.channels = self.block.len();
            let mut frames = self.block.first().map_or(0, Vec::len);
            if self.skip > 0 {
                let drop = self.skip.min(frames as u64) as usize;
                self.skip -= drop as u64;
                for channel in &mut self.block {
                    channel.drain(..drop);
                }
                frames -= drop;
            }
            if let Some(remaining) = self.remaining.as_mut() {
                if (frames as u64) > *remaining {
                    frames = *remaining as usize;
                    for channel in &mut self.block {
                        channel.truncate(frames);
                    }
                }
                *remaining -= frames as u64;
            }
            if frames == 0 {
                continue;
            }
            self.decoded_any = true;
            self.position += frames as u64;
            return Ok(Some(&self.block));
        }
    }

    fn unreadable(&self, reason: &str) -> SampleError {
        SampleError::Unsupported(format!("Cannot read “{}”: {reason}.", self.name))
    }
}

/// Reads only the header of `path`.
pub fn probe_file(path: &Path) -> Result<SourceSpec> {
    Ok(AudioFileReader::open(path)?.spec)
}

fn too_long() -> SampleError {
    SampleError::invalid("Sample must contain audio and be shorter than 100 million frames. Split long recordings before loading them as instruments.")
}

/// Decodes a whole file as a sampler sample: stereo float at the file's sample rate, with the
/// wavetable cycle size and any loop points and root note the file carries.
///
/// `reference` is what the result's `path` is set to (the project's name for the sample);
/// `location` is the file to read. Non-finite samples are replaced by silence. Files longer
/// than 100 million frames are refused.
pub fn decode_file(reference: &str, location: &Path, operation: &mut Operation<'_>) -> Result<DecodedSample> {
    let mut reader = AudioFileReader::open(location)?;
    let total = reader.spec().frames;
    if total.is_some_and(|frames| frames == 0 || frames > MAX_SAMPLE_FRAMES as u64) {
        return Err(too_long());
    }
    let capacity = total.unwrap_or(0) as usize;
    let (mut left, mut right) = (Vec::with_capacity(capacity), Vec::with_capacity(capacity));
    let clean = |value: &f32| if value.is_finite() { *value } else { 0.0 };
    loop {
        operation.check()?;
        let Some(block) = reader.next_block()? else { break };
        let first = &block[0];
        let second = block.get(1).unwrap_or(first);
        if left.len() + first.len() > MAX_SAMPLE_FRAMES {
            return Err(too_long());
        }
        left.extend(first.iter().map(clean));
        right.extend(second.iter().map(clean));
        operation.report(left.len() as u64, total);
    }
    if left.is_empty() {
        return Err(too_long());
    }
    let spec = reader.spec();
    let mut sample = DecodedSample::new(reference, spec.sample_rate as f64, left, right, spec.channels);
    let metadata = if spec.container == "wave" { riff::read_wav_metadata(location) } else { None }.unwrap_or_default();
    sample.cycle_frames = wavetable::detect_frame_size(metadata.declared_frame_size, sample.left.len());
    sample.loops = metadata.loops.into_iter().filter(|l| l.end <= sample.left.len() as u64).collect();
    sample.root_note = metadata.root_note;
    Ok(sample)
}

/// How [`decode_planar`] delivers a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodeOptions {
    /// Convert to this sample rate; `None` keeps the file's rate.
    pub target_rate: Option<u32>,
    /// Converter quality when the rate changes.
    pub quality: ResampleQuality,
    /// Deliver exactly two channels (mono duplicated, extra channels dropped) instead of the
    /// file's own channel count.
    pub stereo: bool,
    /// Stop after this many output frames.
    pub max_frames: Option<u64>,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions { target_rate: None, quality: ResampleQuality::Standard, stereo: false, max_frames: None }
    }
}

/// Decodes a whole file to planar float PCM, optionally converted to a requested sample rate.
///
/// The converter adds no delay: frame `n` of the output is the source at time `n / rate`, and
/// the output has `ceil(source frames * target rate / source rate)` frames.
pub fn decode_planar(path: &Path, options: &DecodeOptions, operation: &mut Operation<'_>) -> Result<PlanarPcm> {
    let mut reader = AudioFileReader::open(path)?;
    let total = reader.spec().frames;
    let source_rate = reader.spec().sample_rate;
    let target_rate = options.target_rate.unwrap_or(source_rate);
    let limit = options.max_frames.unwrap_or(u64::MAX);
    let mut output: Vec<Vec<f32>> = Vec::new();
    let mut resampler: Option<Resampler> = None;
    let mut stereo_block: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    loop {
        operation.check()?;
        let Some(block) = reader.next_block()? else { break };
        let block: &[Vec<f32>] = if options.stereo {
            stereo_block[0].clone_from(&block[0]);
            stereo_block[1].clone_from(block.get(1).unwrap_or(&block[0]));
            &stereo_block
        } else {
            block
        };
        if output.is_empty() {
            output = vec![Vec::new(); block.len()];
            if target_rate != source_rate {
                resampler = Some(Resampler::new(source_rate, target_rate, block.len(), options.quality)?);
            }
        }
        if block.len() != output.len() {
            return Err(SampleError::Unsupported("The file changes its channel count midway, which is not supported.".into()));
        }
        match resampler.as_mut() {
            Some(resampler) => resampler.process(block, &mut output),
            None => output.iter_mut().zip(block).for_each(|(out, input)| out.extend_from_slice(input)),
        }
        operation.report(reader.position(), total);
        if output[0].len() as u64 >= limit {
            break;
        }
    }
    if let Some(resampler) = resampler.as_mut() {
        resampler.finish(&mut output);
    }
    for channel in &mut output {
        channel.truncate(limit.min(usize::MAX as u64) as usize);
    }
    Ok(PlanarPcm { sample_rate: target_rate, channels: output })
}

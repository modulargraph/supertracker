//! WAV (RIFF/WAVE) writing: 16- and 24-bit integer PCM and 32-bit float, any channel count,
//! optional extra chunks, optional TPDF dither for the integer encodings.
//!
//! Reading goes through [`crate::decode`]; [`read_header`] here only answers "what does this
//! file's header claim", which the export pipeline uses to verify what it wrote.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{Result, SampleError};

/// How samples are stored in a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WavEncoding {
    /// Signed 16-bit integer PCM.
    Pcm16,
    /// Signed 24-bit integer PCM.
    Pcm24,
    /// IEEE 32-bit float. Values beyond ±1 are stored as they are.
    Float32,
}

impl WavEncoding {
    /// Bits per sample: 16, 24 or 32.
    pub fn bit_depth(self) -> u32 {
        match self {
            WavEncoding::Pcm16 => 16,
            WavEncoding::Pcm24 => 24,
            WavEncoding::Float32 => 32,
        }
    }

    /// True for [`WavEncoding::Float32`].
    pub fn is_float(self) -> bool {
        self == WavEncoding::Float32
    }

    fn bytes(self) -> usize {
        self.bit_depth() as usize / 8
    }
}

/// Noise added before rounding to an integer encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Dither {
    /// Round to nearest. Deterministic and bit-transparent for audio that already has the
    /// target word length (an integer file decoded and written again is identical).
    #[default]
    None,
    /// Triangular noise of ±1 LSB, which turns quantisation distortion into a constant,
    /// signal-independent noise floor. The usual choice when reducing to 16 bits.
    Tpdf,
}

/// The format of a WAV file to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavSpec {
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channels per frame (1...65535).
    pub channels: u16,
    /// Sample encoding.
    pub encoding: WavEncoding,
    /// Dither for the integer encodings; ignored for float.
    pub dither: Dither,
}

impl WavSpec {
    /// Stereo at `sample_rate` without dither.
    pub fn stereo(sample_rate: u32, encoding: WavEncoding) -> Self {
        WavSpec { sample_rate, channels: 2, encoding, dither: Dither::None }
    }
}

/// An extra RIFF chunk written between `fmt ` and `data` (for example `clm ` or `smpl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavChunk {
    /// Four-character chunk identifier.
    pub id: [u8; 4],
    /// Chunk body; a pad byte is added when its length is odd.
    pub data: Vec<u8>,
}

/// RIFF sizes are 32-bit. Leave room for the header and chunks.
const MAX_DATA_BYTES: u64 = u32::MAX as u64 - 4096;

/// Streaming WAV writer. Create it, call [`WavWriter::write_planar`] any number of times, then
/// [`WavWriter::finalize`], which patches the sizes into the header. A writer that is dropped
/// without `finalize` leaves a file whose header says it is empty.
pub struct WavWriter<W: Write + Seek> {
    out: W,
    spec: WavSpec,
    frames: u64,
    fact_offset: Option<u64>,
    data_size_offset: u64,
    scratch: Vec<u8>,
    dither_state: Vec<u32>,
}

impl WavWriter<BufWriter<File>> {
    /// Creates (or truncates) `path`.
    pub fn create(path: &Path, spec: WavSpec, chunks: &[WavChunk]) -> Result<Self> {
        let file = File::create(path).map_err(|e| SampleError::io("Create audio file", &e))?;
        Self::new(BufWriter::with_capacity(1 << 16, file), spec, chunks)
    }

    /// Creates `path`, failing if something already exists there.
    pub fn create_new(path: &Path, spec: WavSpec, chunks: &[WavChunk]) -> Result<Self> {
        let file = File::options().write(true).create_new(true).open(path).map_err(|e| SampleError::io("Create audio file", &e))?;
        Self::new(BufWriter::with_capacity(1 << 16, file), spec, chunks)
    }
}

impl<W: Write + Seek> WavWriter<W> {
    /// Writes the header and `chunks` to `out`.
    pub fn new(mut out: W, spec: WavSpec, chunks: &[WavChunk]) -> Result<Self> {
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(SampleError::invalid("A WAV file needs at least one channel and a positive sample rate."));
        }
        let bytes = spec.encoding.bytes() as u32;
        let block = bytes * spec.channels as u32;
        let mut header = Vec::with_capacity(128);
        header.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&(if spec.encoding.is_float() { 3u16 } else { 1u16 }).to_le_bytes());
        header.extend_from_slice(&spec.channels.to_le_bytes());
        header.extend_from_slice(&spec.sample_rate.to_le_bytes());
        header.extend_from_slice(&(spec.sample_rate.saturating_mul(block)).to_le_bytes());
        header.extend_from_slice(&(block as u16).to_le_bytes());
        header.extend_from_slice(&(spec.encoding.bit_depth() as u16).to_le_bytes());
        let mut fact_offset = None;
        if spec.encoding.is_float() {
            // Non-PCM formats carry the frame count in a `fact` chunk.
            header.extend_from_slice(b"fact");
            header.extend_from_slice(&4u32.to_le_bytes());
            fact_offset = Some(header.len() as u64);
            header.extend_from_slice(&0u32.to_le_bytes());
        }
        for chunk in chunks {
            let size = u32::try_from(chunk.data.len()).map_err(|_| SampleError::invalid("A WAV metadata chunk is too large."))?;
            header.extend_from_slice(&chunk.id);
            header.extend_from_slice(&size.to_le_bytes());
            header.extend_from_slice(&chunk.data);
            if chunk.data.len() % 2 == 1 {
                header.push(0);
            }
        }
        header.extend_from_slice(b"data");
        let data_size_offset = header.len() as u64;
        header.extend_from_slice(&0u32.to_le_bytes());
        out.write_all(&header).map_err(|e| SampleError::io("Write audio file", &e))?;
        let dither_state = (0..spec.channels as u32).map(|channel| 0x9E37_79B9u32.wrapping_mul(channel + 1) | 1).collect();
        Ok(WavWriter { out, spec, frames: 0, fact_offset, data_size_offset, scratch: Vec::new(), dither_state })
    }

    /// The format given at creation.
    pub fn spec(&self) -> WavSpec {
        self.spec
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Appends frames given as one slice per channel. All slices must have the same length and
    /// there must be exactly `spec.channels` of them.
    ///
    /// Integer encodings round to nearest and clip at full scale; a non-finite sample is written
    /// as silence.
    pub fn write_planar<C: AsRef<[f32]>>(&mut self, channels: &[C]) -> Result<()> {
        let count = self.spec.channels as usize;
        if channels.len() != count {
            return Err(SampleError::invalid("The audio to write does not have the file's channel count."));
        }
        let frames = channels[0].as_ref().len();
        if channels.iter().any(|channel| channel.as_ref().len() != frames) {
            return Err(SampleError::invalid("The channels to write have different lengths."));
        }
        let bytes = self.spec.encoding.bytes();
        let total = (self.frames + frames as u64).checked_mul((bytes * count) as u64);
        if total.is_none_or(|size| size > MAX_DATA_BYTES) {
            return Err(SampleError::invalid("The audio is too long for a WAV file (4 GB). Export a shorter range or a smaller format."));
        }
        self.scratch.clear();
        self.scratch.resize(frames * count * bytes, 0);
        let stride = count * bytes;
        for (index, channel) in channels.iter().enumerate() {
            let source = channel.as_ref();
            let offset = index * bytes;
            match self.spec.encoding {
                WavEncoding::Float32 => {
                    for (frame, &value) in source.iter().enumerate() {
                        let at = frame * stride + offset;
                        self.scratch[at..at + 4].copy_from_slice(&value.to_le_bytes());
                    }
                }
                WavEncoding::Pcm16 => {
                    let state = &mut self.dither_state[index];
                    let dither = self.spec.dither == Dither::Tpdf;
                    for (frame, &value) in source.iter().enumerate() {
                        let sample = quantize(value, 32_768.0, dither, state) as i16;
                        let at = frame * stride + offset;
                        self.scratch[at..at + 2].copy_from_slice(&sample.to_le_bytes());
                    }
                }
                WavEncoding::Pcm24 => {
                    let state = &mut self.dither_state[index];
                    let dither = self.spec.dither == Dither::Tpdf;
                    for (frame, &value) in source.iter().enumerate() {
                        let sample = quantize(value, 8_388_608.0, dither, state);
                        let at = frame * stride + offset;
                        self.scratch[at..at + 3].copy_from_slice(&sample.to_le_bytes()[..3]);
                    }
                }
            }
        }
        self.out.write_all(&self.scratch).map_err(|e| SampleError::io("Write audio file", &e))?;
        self.frames += frames as u64;
        Ok(())
    }

    /// Completes the file: pads the data chunk, writes the final sizes and flushes.
    /// Returns the writer's sink and the number of frames written.
    pub fn finalize(mut self) -> Result<(W, u64)> {
        let failed = |e: std::io::Error| SampleError::io("Finish audio file", &e);
        let data_bytes = self.frames * (self.spec.encoding.bytes() * self.spec.channels as usize) as u64;
        if data_bytes % 2 == 1 {
            self.out.write_all(&[0]).map_err(failed)?;
        }
        let end = self.out.stream_position().map_err(failed)?;
        self.out.seek(SeekFrom::Start(4)).map_err(failed)?;
        self.out.write_all(&((end - 8) as u32).to_le_bytes()).map_err(failed)?;
        if let Some(offset) = self.fact_offset {
            self.out.seek(SeekFrom::Start(offset)).map_err(failed)?;
            self.out.write_all(&(self.frames as u32).to_le_bytes()).map_err(failed)?;
        }
        self.out.seek(SeekFrom::Start(self.data_size_offset)).map_err(failed)?;
        self.out.write_all(&(data_bytes as u32).to_le_bytes()).map_err(failed)?;
        self.out.seek(SeekFrom::Start(end)).map_err(failed)?;
        self.out.flush().map_err(failed)?;
        Ok((self.out, self.frames))
    }
}

/// Scales to the integer range, optionally dithers, rounds to nearest and clips.
#[inline]
fn quantize(value: f32, scale: f64, dither: bool, state: &mut u32) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let mut scaled = value as f64 * scale;
    if dither {
        let mut next = || {
            // xorshift32: cheap, and the sequence depends only on how many samples came before,
            // so the result does not depend on how the audio was split into blocks.
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            *state as f64 / 4_294_967_296.0
        };
        scaled += next() - next();
    }
    scaled.round().clamp(-scale, scale - 1.0) as i32
}

/// Writes a complete WAV file in one call.
pub fn write_wav<C: AsRef<[f32]>>(path: &Path, spec: WavSpec, channels: &[C], chunks: &[WavChunk]) -> Result<()> {
    let mut writer = WavWriter::create(path, spec, chunks)?;
    writer.write_planar(channels)?;
    let (out, _) = writer.finalize()?;
    out.into_inner().map_err(|e| SampleError::io("Finish audio file", e.error()))?;
    Ok(())
}

/// A complete WAV file as bytes.
pub fn wav_bytes<C: AsRef<[f32]>>(spec: WavSpec, channels: &[C], chunks: &[WavChunk]) -> Result<Vec<u8>> {
    let mut writer = WavWriter::new(std::io::Cursor::new(Vec::new()), spec, chunks)?;
    writer.write_planar(channels)?;
    Ok(writer.finalize()?.0.into_inner())
}

/// What a WAV file's header declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavHeader {
    /// Format tag: 1 integer PCM, 3 IEEE float (the sub-format when the file uses the
    /// extensible header).
    pub format_tag: u16,
    /// Channels per frame.
    pub channels: u16,
    /// Frames per second.
    pub sample_rate: u32,
    /// Bits per sample.
    pub bits_per_sample: u16,
    /// Frames in the data chunk.
    pub frames: u64,
}

impl WavHeader {
    /// True when samples are IEEE float.
    pub fn is_float(&self) -> bool {
        self.format_tag == 3
    }
}

/// Reads the `fmt ` and `data` chunk headers of a RIFF/WAVE file. `None` when the file is not
/// a well-formed WAV file.
pub fn read_header(path: &Path) -> Option<WavHeader> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut riff = [0u8; 12];
    file.read_exact(&mut riff).ok()?;
    if &riff[..4] != b"RIFF" || &riff[8..] != b"WAVE" {
        return None;
    }
    let mut offset = 12u64;
    let mut format: Option<(u16, u16, u32, u16, u16)> = None;
    for _ in 0..1024 {
        if offset + 8 > length {
            return None;
        }
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut head = [0u8; 8];
        file.read_exact(&mut head).ok()?;
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64;
        match &head[..4] {
            b"fmt " => {
                let mut body = [0u8; 40];
                let take = size.min(40) as usize;
                if take < 16 {
                    return None;
                }
                file.read_exact(&mut body[..take]).ok()?;
                let mut tag = u16::from_le_bytes([body[0], body[1]]);
                if tag == 0xFFFE && take >= 26 {
                    tag = u16::from_le_bytes([body[24], body[25]]);
                }
                format = Some((
                    tag,
                    u16::from_le_bytes([body[2], body[3]]),
                    u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                    u16::from_le_bytes([body[12], body[13]]),
                    u16::from_le_bytes([body[14], body[15]]),
                ));
            }
            b"data" => {
                let (format_tag, channels, sample_rate, block_align, bits_per_sample) = format?;
                if block_align == 0 || offset + 8 + size > length {
                    return None;
                }
                return Some(WavHeader { format_tag, channels, sample_rate, bits_per_sample, frames: size / block_align as u64 });
            }
            _ => {}
        }
        offset += 8 + size + (size & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout_and_sizes() {
        let bytes = wav_bytes(WavSpec::stereo(48_000, WavEncoding::Pcm16), &[vec![0.5f32, -0.5, 0.25], vec![0.0, 1.0, -1.0]], &[]).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 12);
        let samples: Vec<i16> = bytes[44..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        // Full scale positive clips to the largest representable value.
        assert_eq!(samples, vec![16_384, 0, -16_384, 32_767, 8_192, -32_768]);
    }

    #[test]
    fn odd_data_is_padded_and_extra_chunks_precede_audio() {
        let chunk = WavChunk { id: *b"clm ", data: b"<!>2048".to_vec() };
        let spec = WavSpec { sample_rate: 44_100, channels: 1, encoding: WavEncoding::Pcm24, dither: Dither::None };
        let bytes = wav_bytes(spec, &[vec![0.25f32]], &[chunk]).unwrap();
        assert_eq!(bytes.len() % 2, 0);
        assert_eq!(&bytes[36..40], b"clm ");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 7);
        assert_eq!(&bytes[52..56], b"data");
        assert_eq!(u32::from_le_bytes(bytes[56..60].try_into().unwrap()), 3);
        assert_eq!(&bytes[60..63], &[0x00, 0x00, 0x20]);
    }

    #[test]
    fn float_files_carry_a_fact_chunk_and_keep_overshoot() {
        let bytes = wav_bytes(WavSpec::stereo(96_000, WavEncoding::Float32), &[vec![1.5f32, f32::NAN], vec![-2.0, 0.0]], &[]).unwrap();
        assert_eq!(&bytes[36..40], b"fact");
        assert_eq!(u32::from_le_bytes(bytes[44..48].try_into().unwrap()), 2);
        assert_eq!(f32::from_le_bytes(bytes[56..60].try_into().unwrap()), 1.5);
        assert_eq!(f32::from_le_bytes(bytes[60..64].try_into().unwrap()), -2.0);
    }

    #[test]
    fn tpdf_dither_is_bounded_block_invariant_and_decorrelates_error() {
        let spec = WavSpec { sample_rate: 48_000, channels: 1, encoding: WavEncoding::Pcm16, dither: Dither::Tpdf };
        // A level between two codes: undithered rounding gives a constant, dither a spread whose mean is the level.
        let level = 100.3f32 / 32_768.0;
        let input = vec![level; 40_000];
        let whole = wav_bytes(spec, &[&input], &[]).unwrap();
        let mut writer = WavWriter::new(std::io::Cursor::new(Vec::new()), spec, &[]).unwrap();
        for part in input.chunks(977) {
            writer.write_planar(&[part]).unwrap();
        }
        assert_eq!(writer.finalize().unwrap().0.into_inner(), whole);
        let values: Vec<i16> = whole[44..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert!(values.iter().all(|&v| (99..=102).contains(&v)), "dither stays within ±1.5 LSB");
        let mean = values.iter().map(|&v| v as f64).sum::<f64>() / values.len() as f64;
        assert!((mean - 100.3).abs() < 0.02, "dithered mean {mean} preserves the sub-LSB level");
        let plain = wav_bytes(WavSpec { dither: Dither::None, ..spec }, &[&input], &[]).unwrap();
        assert!(plain[44..].chunks(2).all(|b| i16::from_le_bytes([b[0], b[1]]) == 100));
    }

    #[test]
    fn rejects_mismatched_channels() {
        let mut writer = WavWriter::new(std::io::Cursor::new(Vec::new()), WavSpec::stereo(48_000, WavEncoding::Pcm16), &[]).unwrap();
        assert!(writer.write_planar(&[vec![0.0f32; 4]]).is_err());
        assert!(writer.write_planar(&[vec![0.0f32; 4], vec![0.0f32; 3]]).is_err());
    }
}

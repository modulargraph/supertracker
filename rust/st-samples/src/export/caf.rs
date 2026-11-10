//! The intermediate file an offline render is written to before delivery: 32-bit float PCM in
//! a Core Audio Format (CAF) container.
//!
//! CAF rather than WAV because its sizes are 64-bit (a long stem render at a high rate can pass
//! 4 GB) and because any audio tool can open it when a render needs inspecting. Only the one
//! layout written here is read back: `lpcm`, little-endian float, packed.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{Result, SampleError};

/// `kCAFLinearPCMFormatFlagIsFloat | kCAFLinearPCMFormatFlagIsLittleEndian`.
const FLOAT_LITTLE_ENDIAN: u32 = 3;
/// Bytes before the first sample: file header, `desc` chunk, `data` header and edit count.
const AUDIO_OFFSET: u64 = 8 + 12 + 32 + 12 + 4;
const DATA_SIZE_OFFSET: u64 = 8 + 12 + 32 + 4;

/// Writes a render file block by block. Call [`RenderFileWriter::finalize`] when done.
pub struct RenderFileWriter {
    out: BufWriter<File>,
    channels: usize,
    frames: u64,
    scratch: Vec<u8>,
}

impl RenderFileWriter {
    /// Creates `path` (it must not exist) for `channels` channels at `sample_rate`.
    pub fn create(path: &Path, sample_rate: u32, channels: usize) -> Result<Self> {
        if sample_rate == 0 || channels == 0 || channels > 64 {
            return Err(SampleError::invalid("A render file needs a positive sample rate and 1 to 64 channels."));
        }
        let file = File::options().write(true).create_new(true).open(path).map_err(|e| SampleError::io("Create the render file", &e))?;
        let mut out = BufWriter::with_capacity(1 << 16, file);
        let mut header = Vec::with_capacity(AUDIO_OFFSET as usize);
        header.extend_from_slice(b"caff");
        header.extend_from_slice(&1u16.to_be_bytes());
        header.extend_from_slice(&0u16.to_be_bytes());
        header.extend_from_slice(b"desc");
        header.extend_from_slice(&32i64.to_be_bytes());
        header.extend_from_slice(&(sample_rate as f64).to_be_bytes());
        header.extend_from_slice(b"lpcm");
        header.extend_from_slice(&FLOAT_LITTLE_ENDIAN.to_be_bytes());
        header.extend_from_slice(&(4 * channels as u32).to_be_bytes());
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&(channels as u32).to_be_bytes());
        header.extend_from_slice(&32u32.to_be_bytes());
        header.extend_from_slice(b"data");
        // -1: the audio runs to the end of the file. Replaced by the real size on finalize, so a
        // render that was interrupted is still readable.
        header.extend_from_slice(&(-1i64).to_be_bytes());
        header.extend_from_slice(&0u32.to_be_bytes());
        out.write_all(&header).map_err(|e| SampleError::io("Write the render file", &e))?;
        Ok(RenderFileWriter { out, channels, frames: 0, scratch: Vec::new() })
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Appends frames given as one slice per channel (equal lengths).
    pub fn write_planar<C: AsRef<[f32]>>(&mut self, channels: &[C]) -> Result<()> {
        if channels.len() != self.channels {
            return Err(SampleError::invalid("The audio to write does not have the render file's channel count."));
        }
        let frames = channels[0].as_ref().len();
        if channels.iter().any(|channel| channel.as_ref().len() != frames) {
            return Err(SampleError::invalid("The channels to write have different lengths."));
        }
        self.scratch.clear();
        self.scratch.resize(frames * self.channels * 4, 0);
        for (index, channel) in channels.iter().enumerate() {
            for (frame, value) in channel.as_ref().iter().enumerate() {
                let at = (frame * self.channels + index) * 4;
                self.scratch[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        self.out.write_all(&self.scratch).map_err(|e| SampleError::io("Write the render file", &e))?;
        self.frames += frames as u64;
        Ok(())
    }

    /// Writes the final size and flushes. Returns the number of frames in the file.
    pub fn finalize(mut self) -> Result<u64> {
        let failed = |e: std::io::Error| SampleError::io("Finish the render file", &e);
        let size = 4 + self.frames * self.channels as u64 * 4;
        self.out.seek(SeekFrom::Start(DATA_SIZE_OFFSET)).map_err(failed)?;
        self.out.write_all(&(size as i64).to_be_bytes()).map_err(failed)?;
        self.out.flush().map_err(failed)?;
        Ok(self.frames)
    }
}

/// Reads a render file written by [`RenderFileWriter`].
pub struct RenderFileReader {
    input: BufReader<File>,
    sample_rate: u32,
    channels: usize,
    frames: u64,
    position: u64,
    scratch: Vec<u8>,
}

impl RenderFileReader {
    /// Opens `path` and checks that it is a float render file.
    pub fn open(path: &Path) -> Result<Self> {
        let wrong = || SampleError::invalid("The intermediate render is not a 32-bit float CAF file.");
        let file = File::open(path).map_err(|e| SampleError::io("Open the render file", &e))?;
        let length = file.metadata().map_err(|e| SampleError::io("Open the render file", &e))?.len();
        let mut input = BufReader::with_capacity(1 << 16, file);
        let mut header = [0u8; AUDIO_OFFSET as usize];
        input.read_exact(&mut header).map_err(|_| wrong())?;
        let be32 = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
        let rate = f64::from_be_bytes(header[20..28].try_into().unwrap());
        let channels = be32(44) as usize;
        let layout_ok = &header[..4] == b"caff"
            && &header[8..12] == b"desc"
            && i64::from_be_bytes(header[12..20].try_into().unwrap()) == 32
            && &header[28..32] == b"lpcm"
            && be32(32) == FLOAT_LITTLE_ENDIAN
            && be32(36) as usize == 4 * channels
            && be32(40) == 1
            && (1..=64).contains(&channels)
            && be32(48) == 32
            && &header[52..56] == b"data"
            && rate.fract() == 0.0
            && (1.0..=768_000.0).contains(&rate);
        if !layout_ok {
            return Err(wrong());
        }
        let declared = i64::from_be_bytes(header[56..64].try_into().unwrap());
        let available = length - AUDIO_OFFSET;
        let bytes = if declared < 0 { available } else { (declared as u64).saturating_sub(4).min(available) };
        Ok(RenderFileReader { input, sample_rate: rate as u32, channels, frames: bytes / (4 * channels as u64), position: 0, scratch: Vec::new() })
    }

    /// Frames per second.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channels per frame.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames in the file.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Frames read so far.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Reads up to `maximum` frames into `output` (one vector per channel, replaced). Returns
    /// the number of frames read; 0 at the end.
    pub fn read_planar(&mut self, maximum: usize, output: &mut [Vec<f32>]) -> Result<usize> {
        let count = (self.frames - self.position).min(maximum as u64) as usize;
        self.scratch.resize(count * self.channels * 4, 0);
        self.input.read_exact(&mut self.scratch).map_err(|_| SampleError::invalid("The intermediate render changed or ended early."))?;
        for (index, channel) in output.iter_mut().enumerate().take(self.channels) {
            channel.clear();
            channel.extend(self.scratch.chunks_exact(4).skip(index).step_by(self.channels).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        }
        self.position += count as u64;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_file, Operation};

    #[test]
    fn round_trip_is_bit_exact_and_other_tools_agree() {
        let dir = std::env::temp_dir().join(format!("st-samples-caf-{}-{}", std::process::id(), crate::paths::unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("render.caf");
        let left: Vec<f32> = (0..10_007).map(|i| (i as f32 * 0.01).sin() * 1.3).collect();
        let right: Vec<f32> = left.iter().map(|v| -v * 0.5).collect();
        let mut writer = RenderFileWriter::create(&path, 48_000, 2).unwrap();
        for (l, r) in left.chunks(999).zip(right.chunks(999)) {
            writer.write_planar(&[l, r]).unwrap();
        }
        assert_eq!(writer.finalize().unwrap(), 10_007);
        assert!(RenderFileWriter::create(&path, 48_000, 2).is_err(), "an existing render is never overwritten");

        let mut reader = RenderFileReader::open(&path).unwrap();
        assert_eq!((reader.sample_rate(), reader.channels(), reader.frames()), (48_000, 2, 10_007));
        let (mut out, mut all) = (vec![Vec::new(), Vec::new()], vec![Vec::new(), Vec::new()]);
        while reader.read_planar(4096, &mut out).unwrap() > 0 {
            all[0].extend_from_slice(&out[0]);
            all[1].extend_from_slice(&out[1]);
        }
        assert!(all[0] == left && all[1] == right, "float samples survive unchanged, including values beyond full scale");

        // The general decoder reads the same file: it is a real CAF file.
        let decoded = decode_file("render", &path, &mut Operation::new()).unwrap();
        assert!(decoded.left == left && decoded.right == right && decoded.sample_rate == 48_000.0);

        // An unfinished render (size still -1) reads to the end of the file.
        let open = dir.join("open.caf");
        let mut writer = RenderFileWriter::create(&open, 44_100, 1).unwrap();
        writer.write_planar(&[&left[..100]]).unwrap();
        writer.out.flush().unwrap();
        assert_eq!(RenderFileReader::open(&open).unwrap().frames(), 100);

        std::fs::write(dir.join("bad.caf"), b"caff but not really a core audio file at all, just text that is long enough").unwrap();
        assert!(RenderFileReader::open(&dir.join("bad.caf")).is_err());
        assert!(RenderFileReader::open(&dir.join("missing.caf")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}

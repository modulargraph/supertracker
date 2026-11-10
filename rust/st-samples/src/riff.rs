//! Sampler metadata stored in WAV chunks that the decoder does not surface: loop points and
//! root note (`smpl`, `inst`) and wavetable cycle size (`clm `, `srge`, `uhWT`).
//!
//! Only chunk headers and these small chunk bodies are read; the audio is never touched.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;

use crate::wav::WavChunk;

/// How a loop plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopKind {
    /// Start to end, then again from the start.
    Forward,
    /// Start to end, then backwards to the start, and so on.
    PingPong,
    /// End to start.
    Reverse,
}

/// A loop stored in a file's `smpl` chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleLoop {
    /// First frame of the loop.
    pub start: u64,
    /// One past the last frame of the loop (the chunk itself stores the last frame).
    pub end: u64,
    /// Loop direction.
    pub kind: LoopKind,
    /// Number of repetitions; 0 means "until released".
    pub play_count: u32,
}

/// Metadata found in a WAV file's chunks. Every field is `None`/empty when the chunk is absent.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WavMetadata {
    /// Wavetable cycle size in frames declared by a `clm ` ("<!>2048 ..."), `srge` or `uhWT`
    /// chunk, in that order of precedence. Not validated against the file length.
    pub declared_frame_size: Option<usize>,
    /// Loops from the `smpl` chunk, in file order.
    pub loops: Vec<SampleLoop>,
    /// MIDI note at which the sample plays at its recorded pitch (`smpl` unity note, else `inst`).
    pub root_note: Option<u8>,
    /// Fine tuning in cents (`smpl` pitch fraction, else `inst` fine tune), -50...+100.
    pub fine_tune_cents: Option<f64>,
}

/// Frame sizes a wavetable may declare.
pub(crate) const FRAME_SIZE_RANGE: std::ops::RangeInclusive<usize> = 16..=8192;

/// "<!>2048 01000000 wavetable (vendor)" -> 2048. `None` when the text does not follow the
/// convention or the size is outside 16...8192.
pub fn frame_size_from_clm(text: &str) -> Option<usize> {
    let rest = text.strip_prefix("<!>")?;
    let digits: String = rest.chars().take_while(|c| c.is_numeric()).collect();
    let value: usize = digits.parse().ok()?;
    FRAME_SIZE_RANGE.contains(&value).then_some(value)
}

/// Reads the sampler metadata of the WAV file at `path`. `None` when the file cannot be read
/// or is not RIFF/WAVE; a WAV file without any of the chunks gives an empty [`WavMetadata`].
pub fn read_wav_metadata(path: &Path) -> Option<WavMetadata> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut header = [0u8; 12];
    file.read_exact(&mut header).ok()?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return None;
    }
    let mut metadata = WavMetadata::default();
    let (mut clm, mut srge, mut uhwt) = (None, None, None);
    let (mut smpl_note, mut inst_note) = (None, None);
    let (mut smpl_tune, mut inst_tune) = (None, None);
    let mut offset = 12u64;
    // Metadata chunks precede or follow the audio; a bound keeps a damaged file from looping.
    for _ in 0..1024 {
        if offset + 8 > length || file.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
        let mut head = [0u8; 8];
        if file.read_exact(&mut head).is_err() {
            break;
        }
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64;
        let mut body = |limit: u64| -> Option<Vec<u8>> {
            if size > limit {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            file.read_exact(&mut data).ok()?;
            Some(data)
        };
        let u32_at = |data: &[u8], at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        match &head[..4] {
            b"clm " => clm = body(4096).and_then(|data| frame_size_from_clm(&String::from_utf8_lossy(&data))),
            b"srge" => {
                // Surge: version, then the cycle length.
                srge = body(64).filter(|data| data.len() >= 8).map(|data| u32_at(&data, 4) as usize).filter(|size| FRAME_SIZE_RANGE.contains(size));
            }
            // u-he tables have no size field: their cycles are 2048 frames.
            b"uhWT" => uhwt = Some(2048),
            b"smpl" => {
                if let Some(data) = body(1 << 16).filter(|data| data.len() >= 36) {
                    let note = u32_at(&data, 12);
                    if note <= 127 {
                        smpl_note = Some(note as u8);
                    }
                    // The fraction is a 32-bit fixed point part of a semitone.
                    smpl_tune = Some(u32_at(&data, 16) as f64 / 4_294_967_296.0 * 100.0);
                    let count = u32_at(&data, 28) as usize;
                    for index in 0..count.min((data.len() - 36) / 24) {
                        let at = 36 + index * 24;
                        let kind = match u32_at(&data, at + 4) {
                            1 => LoopKind::PingPong,
                            2 => LoopKind::Reverse,
                            _ => LoopKind::Forward,
                        };
                        let (start, last) = (u32_at(&data, at + 8) as u64, u32_at(&data, at + 12) as u64);
                        if last >= start {
                            metadata.loops.push(SampleLoop { start, end: last + 1, kind, play_count: u32_at(&data, at + 20) });
                        }
                    }
                }
            }
            b"inst" => {
                if let Some(data) = body(64).filter(|data| data.len() >= 2) {
                    if data[0] <= 127 {
                        inst_note = Some(data[0]);
                    }
                    inst_tune = Some(data[1] as i8 as f64);
                }
            }
            _ => {}
        }
        offset += 8 + size + (size & 1);
    }
    metadata.declared_frame_size = clm.or(srge).or(uhwt);
    metadata.root_note = smpl_note.or(inst_note);
    metadata.fine_tune_cents = smpl_tune.or(inst_tune);
    Some(metadata)
}

/// A `clm ` chunk declaring a wavetable cycle of `frame_size` frames.
pub fn clm_chunk(frame_size: usize) -> WavChunk {
    WavChunk { id: *b"clm ", data: format!("<!>{frame_size} 10000000 wavetable (SuperTracker)").into_bytes() }
}

/// A `smpl` chunk with the given root note and loops.
pub fn smpl_chunk(sample_rate: u32, root_note: u8, loops: &[SampleLoop]) -> WavChunk {
    let mut data = Vec::with_capacity(36 + loops.len() * 24);
    let period = if sample_rate > 0 { 1_000_000_000 / sample_rate } else { 0 };
    for value in [0, 0, period, root_note as u32, 0, 0, 0, loops.len() as u32, 0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for (index, item) in loops.iter().enumerate() {
        let kind = match item.kind {
            LoopKind::Forward => 0u32,
            LoopKind::PingPong => 1,
            LoopKind::Reverse => 2,
        };
        let last = item.end.saturating_sub(1).min(u32::MAX as u64) as u32;
        for value in [index as u32, kind, item.start.min(u32::MAX as u64) as u32, last, 0, item.play_count] {
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    WavChunk { id: *b"smpl", data }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::{wav_bytes, Dither, WavEncoding, WavSpec};

    fn temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("st-samples-riff-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.wav");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    const MONO: WavSpec = WavSpec { sample_rate: 44_100, channels: 1, encoding: WavEncoding::Pcm16, dither: Dither::None };

    #[test]
    fn clm_text_parsing() {
        assert_eq!(frame_size_from_clm("<!>2048 01000000 wavetable (vendor)"), Some(2048));
        assert_eq!(frame_size_from_clm("<!>256"), Some(256));
        assert_eq!(frame_size_from_clm("2048"), None);
        assert_eq!(frame_size_from_clm("<!>8 x"), None);
        assert_eq!(frame_size_from_clm("<!>16384"), None);
        assert_eq!(frame_size_from_clm("<!> 2048"), None);
    }

    #[test]
    fn reads_loop_root_note_and_cycle_chunks() {
        let loops = [
            SampleLoop { start: 100, end: 900, kind: LoopKind::Forward, play_count: 0 },
            SampleLoop { start: 10, end: 20, kind: LoopKind::PingPong, play_count: 3 },
        ];
        let mut inst = WavChunk { id: *b"inst", data: vec![60, (-12i8) as u8, 0, 0, 127, 1, 127] };
        let bytes = wav_bytes(MONO, &[vec![0.0f32; 1000]], &[smpl_chunk(44_100, 57, &loops), inst.clone(), clm_chunk(512)]).unwrap();
        let metadata = read_wav_metadata(&temp("all", &bytes)).unwrap();
        assert_eq!(metadata.loops, loops);
        assert_eq!(metadata.root_note, Some(57), "smpl wins over inst");
        assert_eq!(metadata.fine_tune_cents, Some(0.0));
        assert_eq!(metadata.declared_frame_size, Some(512));

        inst.data[0] = 64;
        let bytes = wav_bytes(MONO, &[vec![0.0f32; 1000]], &[inst]).unwrap();
        let metadata = read_wav_metadata(&temp("inst", &bytes)).unwrap();
        assert_eq!((metadata.root_note, metadata.fine_tune_cents), (Some(64), Some(-12.0)));
        assert!(metadata.loops.is_empty() && metadata.declared_frame_size.is_none());
    }

    #[test]
    fn surge_and_uhe_cycle_chunks_and_trailing_chunks() {
        let mut srge = Vec::new();
        srge.extend_from_slice(&1u32.to_le_bytes());
        srge.extend_from_slice(&1024u32.to_le_bytes());
        let bytes = wav_bytes(MONO, &[vec![0.0f32; 2048]], &[WavChunk { id: *b"srge", data: srge }]).unwrap();
        assert_eq!(read_wav_metadata(&temp("srge", &bytes)).unwrap().declared_frame_size, Some(1024));
        let bytes = wav_bytes(MONO, &[vec![0.0f32; 4096]], &[WavChunk { id: *b"uhWT", data: vec![0; 16] }]).unwrap();
        assert_eq!(read_wav_metadata(&temp("uhwt", &bytes)).unwrap().declared_frame_size, Some(2048));

        // A chunk after the audio (and after an odd-sized chunk's pad byte) is still found.
        let mut bytes = wav_bytes(MONO, &[vec![0.0f32; 7]], &[WavChunk { id: *b"odd ", data: vec![1, 2, 3] }]).unwrap();
        let clm = clm_chunk(256);
        bytes.extend_from_slice(&clm.id);
        bytes.extend_from_slice(&(clm.data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&clm.data);
        assert_eq!(read_wav_metadata(&temp("trailing", &bytes)).unwrap().declared_frame_size, Some(256));
    }

    #[test]
    fn non_wav_and_damaged_files() {
        assert!(read_wav_metadata(&temp("text", b"not a wave file at all")).is_none());
        assert!(read_wav_metadata(Path::new("/nonexistent/st-samples.wav")).is_none());
        let mut bytes = wav_bytes(MONO, &[vec![0.0f32; 8]], &[clm_chunk(2048)]).unwrap();
        bytes.truncate(50);
        assert_eq!(read_wav_metadata(&temp("cut", &bytes)).unwrap(), WavMetadata::default());
    }
}

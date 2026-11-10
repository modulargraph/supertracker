//! The one thing the MP4 demuxer does not report: which part of the decoded audio is meant to
//! be played. AAC encoders prepend "priming" frames (1024 or 2112 are common) and pad the end to
//! a whole packet; the track's edit list says where the real audio starts and how long it is.
//! Without it every AAC sample would start 20-50 ms late.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The playable part of an MP4 audio track, in frames at the track's sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Edit {
    /// Decoded frames to discard at the start.
    pub start: u64,
    /// Frames to play after that, when the edit states a duration.
    pub frames: Option<u64>,
}

struct Atom {
    kind: [u8; 4],
    body: u64,
    end: u64,
}

/// The child atoms in `[start, end)`.
fn atoms(file: &mut File, start: u64, end: u64) -> Vec<Atom> {
    let mut result = Vec::new();
    let mut offset = start;
    // The bound keeps a damaged file from being walked forever.
    while offset + 8 <= end && result.len() < 4096 {
        let mut head = [0u8; 16];
        if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut head[..8]).is_err() {
            break;
        }
        let mut size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let mut header = 8;
        if size == 1 {
            if file.read_exact(&mut head[8..]).is_err() {
                break;
            }
            size = u64::from_be_bytes(head[8..16].try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = end - offset;
        }
        if size < header || offset + size > end {
            break;
        }
        result.push(Atom { kind: [head[4], head[5], head[6], head[7]], body: offset + header, end: offset + size });
        offset += size;
    }
    result
}

fn body(file: &mut File, atom: &Atom, limit: u64) -> Option<Vec<u8>> {
    let size = atom.end - atom.body;
    if size > limit {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    file.seek(SeekFrom::Start(atom.body)).ok()?;
    file.read_exact(&mut data).ok()?;
    Some(data)
}

fn be32(data: &[u8], at: usize) -> Option<u64> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as u64)
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// The timescale of an `mvhd` or `mdhd` atom (both share the layout up to that field).
fn timescale(data: &[u8]) -> Option<u64> {
    let scale = if *data.first()? == 1 { be32(data, 20)? } else { be32(data, 12)? };
    (scale > 0).then_some(scale)
}

/// Reads the edit list of the first audio track of the MP4 file at `path`. `None` when the
/// file has no (usable) edit list, in which case everything that decodes is played.
pub(crate) fn read_edit(path: &Path, sample_rate: u32) -> Option<Edit> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let top = atoms(&mut file, 0, length);
    let moov = top.iter().find(|atom| &atom.kind == b"moov")?;
    let children = atoms(&mut file, moov.body, moov.end);
    let mvhd = children.iter().find(|atom| &atom.kind == b"mvhd")?;
    let movie_scale = timescale(&body(&mut file, mvhd, 256)?)?;
    for trak in children.iter().filter(|atom| &atom.kind == b"trak") {
        let parts = atoms(&mut file, trak.body, trak.end);
        let Some(mdia) = parts.iter().find(|atom| &atom.kind == b"mdia") else { continue };
        let media = atoms(&mut file, mdia.body, mdia.end);
        let is_audio = media
            .iter()
            .find(|atom| &atom.kind == b"hdlr")
            .and_then(|atom| body(&mut file, atom, 1024))
            .is_some_and(|data| data.get(8..12) == Some(b"soun"));
        if !is_audio {
            continue;
        }
        let media_scale = media.iter().find(|atom| &atom.kind == b"mdhd").and_then(|atom| body(&mut file, atom, 256)).and_then(|data| timescale(&data))?;
        let edts = parts.iter().find(|atom| &atom.kind == b"edts")?;
        let elst = atoms(&mut file, edts.body, edts.end).into_iter().find(|atom| &atom.kind == b"elst")?;
        let data = body(&mut file, &elst, 1 << 16)?;
        let version = *data.first()?;
        let count = be32(&data, 4)?;
        let mut at = 8;
        for _ in 0..count.min(64) {
            let (duration, media_time);
            if version == 1 {
                duration = be64(&data, at)?;
                media_time = be64(&data, at + 8)? as i64;
                at += 20;
            } else {
                duration = be32(&data, at)?;
                media_time = be32(&data, at + 4)? as u32 as i32 as i64;
                at += 12;
            }
            // A media time of -1 is an empty edit (a pause before the track starts): not audio.
            if media_time < 0 {
                continue;
            }
            let rate = sample_rate as u128;
            let start = (media_time as u128 * rate / media_scale as u128) as u64;
            let frames = (duration > 0).then(|| ((duration as u128 * rate + movie_scale as u128 / 2) / movie_scale as u128) as u64);
            return Some(Edit { start, frames });
        }
        return None;
    }
    None
}

//! Length of an audio file from its header, without decoding audio.
//!
//! Understands WAV (and RF64), AIFF/AIFC, FLAC, CAF, Ogg Vorbis/Opus, MPEG-4 audio and MP3.
//! Every read is bounded and seeks past sample data; a file that does not parse gives `None`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// What a header says about an audio file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioInfo {
    /// Length in seconds.
    pub duration: f64,
    /// Length in sample frames, where the header states it exactly.
    pub frames: Option<u64>,
    /// Sample rate in Hz.
    pub sample_rate: f64,
}

/// The header information of the audio file at `path`, or `None` when it cannot be read as
/// one of the supported formats.
pub fn audio_info(path: &Path) -> Option<AudioInfo> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mut magic = [0u8; 12];
    let got = read_up_to(&mut file, &mut magic)?;
    if got < 12 {
        return None;
    }
    let info = match &magic[0..4] {
        b"RIFF" | b"RF64" if &magic[8..12] == b"WAVE" => wav(&mut file, size, &magic[0..4] == b"RF64"),
        b"FORM" if &magic[8..12] == b"AIFF" || &magic[8..12] == b"AIFC" => aiff(&mut file, size),
        b"fLaC" => flac(&mut file, 4),
        b"caff" => caf(&mut file, size),
        b"OggS" => ogg(&mut file, size),
        _ if &magic[4..8] == b"ftyp" => mp4(&mut file, size),
        _ if &magic[0..3] == b"ID3" => {
            let skip = 10 + synchsafe(&magic[6..10]);
            let mut next = [0u8; 4];
            file.seek(SeekFrom::Start(skip)).ok()?;
            if read_up_to(&mut file, &mut next)? == 4 && &next == b"fLaC" {
                flac(&mut file, skip + 4)
            } else {
                mp3(&mut file, size, skip)
            }
        }
        _ if magic[0] == 0xff && magic[1] & 0xe0 == 0xe0 => mp3(&mut file, size, 0),
        _ => None,
    }?;
    (info.duration.is_finite() && info.duration >= 0.0 && info.sample_rate > 0.0).then_some(info)
}

/// Length in seconds of the audio file at `path`, from its header.
pub fn header_duration(path: &Path) -> Option<f64> {
    audio_info(path).map(|info| info.duration)
}

fn read_up_to(file: &mut File, buffer: &mut [u8]) -> Option<usize> {
    let mut total = 0;
    while total < buffer.len() {
        match file.read(&mut buffer[total..]) {
            Ok(0) => break,
            Ok(count) => total += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(total)
}

fn read_at(file: &mut File, offset: u64, count: usize) -> Option<Vec<u8>> {
    if count > 1 << 20 {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0u8; count];
    (read_up_to(file, &mut buffer)? == count).then_some(buffer)
}

fn le16(bytes: &[u8]) -> u64 {
    u64::from(u16::from_le_bytes([bytes[0], bytes[1]]))
}
fn le32(bytes: &[u8]) -> u64 {
    u64::from(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
fn le64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes[0..8].try_into().unwrap_or([0; 8]))
}
fn be32(bytes: &[u8]) -> u64 {
    u64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
fn be64(bytes: &[u8]) -> u64 {
    u64::from_be_bytes(bytes[0..8].try_into().unwrap_or([0; 8]))
}
fn synchsafe(bytes: &[u8]) -> u64 {
    bytes.iter().take(4).fold(0u64, |value, byte| value << 7 | u64::from(byte & 0x7f))
}

fn wav(file: &mut File, size: u64, rf64: bool) -> Option<AudioInfo> {
    let (mut offset, mut format, mut data, mut fact, mut data64) = (12u64, None, None, None, None);
    for _ in 0..4096 {
        if offset + 8 > size {
            break;
        }
        let header = read_at(file, offset, 8)?;
        let mut length = le32(&header[4..8]);
        let start = offset + 8;
        match &header[0..4] {
            b"fmt " if length >= 16 => {
                let body = read_at(file, start, 16)?;
                format = Some((le16(&body[0..2]), le32(&body[4..8]), le32(&body[8..12]), le16(&body[12..14])));
            }
            b"ds64" if length >= 24 => {
                let body = read_at(file, start, 24)?;
                data64 = Some((le64(&body[8..16]), le64(&body[16..24])));
            }
            b"fact" if length >= 4 => fact = Some(le32(&read_at(file, start, 4)?)),
            b"data" => {
                if rf64 && length == 0xffff_ffff {
                    length = data64.map(|sizes| sizes.0).unwrap_or(length);
                }
                data = Some(length.min(size.saturating_sub(start)));
                if format.is_some() {
                    break;
                }
            }
            _ => {}
        }
        offset = start + length + (length & 1);
    }
    let (tag, rate, byte_rate, block) = format?;
    let data = data?;
    if rate == 0 {
        return None;
    }
    // PCM, IEEE float and extensible store whole frames; compressed forms state a sample count.
    if matches!(tag, 1 | 3 | 0xfffe) && block > 0 {
        let frames = data / block;
        return Some(AudioInfo { duration: frames as f64 / rate as f64, frames: Some(frames), sample_rate: rate as f64 });
    }
    if let Some(frames) = fact.filter(|frames| *frames > 0) {
        return Some(AudioInfo { duration: frames as f64 / rate as f64, frames: Some(frames), sample_rate: rate as f64 });
    }
    (byte_rate > 0).then(|| AudioInfo { duration: data as f64 / byte_rate as f64, frames: None, sample_rate: rate as f64 })
}

/// An 80-bit IEEE 754 extended value (AIFF sample rates).
fn extended(bytes: &[u8]) -> f64 {
    let exponent = (i32::from(bytes[0] & 0x7f) << 8 | i32::from(bytes[1])) - 16383;
    let mantissa = be64(&bytes[2..10]);
    let value = mantissa as f64 * 2f64.powi(exponent - 63);
    if bytes[0] & 0x80 != 0 {
        -value
    } else {
        value
    }
}

fn aiff(file: &mut File, size: u64) -> Option<AudioInfo> {
    let mut offset = 12u64;
    for _ in 0..4096 {
        if offset + 8 > size {
            break;
        }
        let header = read_at(file, offset, 8)?;
        let length = be32(&header[4..8]);
        if &header[0..4] == b"COMM" && length >= 18 {
            let body = read_at(file, offset + 8, 18)?;
            let frames = be32(&body[2..6]);
            let rate = extended(&body[8..18]);
            if !(rate.is_finite() && rate > 0.0) {
                return None;
            }
            return Some(AudioInfo { duration: frames as f64 / rate, frames: Some(frames), sample_rate: rate });
        }
        offset += 8 + length + (length & 1);
    }
    None
}

fn flac(file: &mut File, offset: u64) -> Option<AudioInfo> {
    let block = read_at(file, offset, 4 + 34)?;
    if block[0] & 0x7f != 0 {
        return None; // The first metadata block must be STREAMINFO.
    }
    let info = &block[4..];
    let packed = be64(&info[10..18]);
    let rate = packed >> 44;
    let frames = packed & 0xf_ffff_ffff;
    (rate > 0).then(|| AudioInfo { duration: frames as f64 / rate as f64, frames: Some(frames), sample_rate: rate as f64 })
}

fn caf(file: &mut File, size: u64) -> Option<AudioInfo> {
    let (mut offset, mut rate, mut bytes_per_packet, mut frames_per_packet, mut frames, mut data) = (8u64, 0.0f64, 0u64, 0u64, None, None);
    for _ in 0..4096 {
        if offset + 12 > size {
            break;
        }
        let header = read_at(file, offset, 12)?;
        let length = be64(&header[4..12]);
        let start = offset + 12;
        match &header[0..4] {
            b"desc" if length >= 32 => {
                let body = read_at(file, start, 32)?;
                rate = f64::from_bits(be64(&body[0..8]));
                bytes_per_packet = be32(&body[16..20]);
                frames_per_packet = be32(&body[20..24]);
            }
            b"pakt" if length >= 24 => {
                let body = read_at(file, start, 24)?;
                frames = Some(be64(&body[8..16]));
            }
            b"data" => {
                // A size of -1 means "to the end of the file"; the chunk starts with an edit count.
                let length = if length == u64::MAX { size.saturating_sub(start) } else { length };
                data = Some(length.saturating_sub(4));
                if length == u64::MAX {
                    break;
                }
            }
            _ => {}
        }
        if length > size {
            break;
        }
        offset = start + length;
    }
    if !(rate.is_finite() && rate > 0.0) {
        return None;
    }
    let frames = frames.filter(|frames| *frames > 0).or_else(|| (bytes_per_packet > 0 && frames_per_packet > 0).then(|| data.unwrap_or(0) / bytes_per_packet * frames_per_packet))?;
    Some(AudioInfo { duration: frames as f64 / rate, frames: Some(frames), sample_rate: rate })
}

fn ogg(file: &mut File, size: u64) -> Option<AudioInfo> {
    let head = read_at(file, 0, 64.min(size as usize))?;
    let segments = *head.get(26)? as usize;
    let packet = 27 + segments;
    let body = head.get(packet..)?;
    let (rate, pre_skip) = if body.len() >= 16 && &body[0..7] == b"\x01vorbis" {
        (le32(&body[12..16]), 0)
    } else if body.len() >= 12 && &body[0..8] == b"OpusHead" {
        (48_000, le16(&body[10..12]))
    } else {
        return None;
    };
    if rate == 0 {
        return None;
    }
    // The last page's granule position is the stream length in samples.
    let tail_length = size.min(65_536 + 27 + 255);
    let tail = read_at(file, size - tail_length, tail_length as usize)?;
    let position = (0..tail.len().saturating_sub(14)).rev().find(|&at| &tail[at..at + 4] == b"OggS" && tail[at + 4] == 0)?;
    let granule = le64(&tail[position + 6..position + 14]);
    if granule == u64::MAX {
        return None;
    }
    let frames = granule.saturating_sub(pre_skip);
    Some(AudioInfo { duration: frames as f64 / rate as f64, frames: Some(frames), sample_rate: rate as f64 })
}

fn mp4(file: &mut File, size: u64) -> Option<AudioInfo> {
    // Top-level boxes until `moov`, then its `mvhd`.
    let mut offset = 0u64;
    for _ in 0..4096 {
        if offset + 8 > size {
            break;
        }
        let header = read_at(file, offset, 16.min((size - offset) as usize))?;
        let mut length = be32(&header[0..4]);
        let mut header_size = 8;
        if length == 1 && header.len() >= 16 {
            length = be64(&header[8..16]);
            header_size = 16;
        } else if length == 0 {
            length = size - offset;
        }
        if length < header_size {
            return None;
        }
        if &header[4..8] == b"moov" {
            let end = (offset + length).min(size);
            let mut inner = offset + header_size;
            for _ in 0..256 {
                if inner + 8 > end {
                    break;
                }
                let child = read_at(file, inner, 8)?;
                let child_length = be32(&child[0..4]).max(8);
                if &child[4..8] == b"mvhd" {
                    let body = read_at(file, inner + 8, 32.min((end - inner - 8) as usize))?;
                    let (timescale, duration) = if body.first() == Some(&1) && body.len() >= 32 {
                        (be32(&body[20..24]), be64(&body[24..32]))
                    } else if body.len() >= 20 {
                        (be32(&body[12..16]), be32(&body[16..20]))
                    } else {
                        return None;
                    };
                    return (timescale > 0).then(|| AudioInfo { duration: duration as f64 / timescale as f64, frames: None, sample_rate: timescale as f64 });
                }
                inner += child_length;
            }
            return None;
        }
        offset += length;
    }
    None
}

fn mp3(file: &mut File, size: u64, start: u64) -> Option<AudioInfo> {
    // The first frame header within 64 KiB of the audio start.
    let window = read_at(file, start, (size.saturating_sub(start)).min(65_536) as usize)?;
    const BITRATES_V1_L3: [u64; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const BITRATES_V2_L3: [u64; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    const BITRATES_V1_L2: [u64; 16] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0];
    const BITRATES_V1_L1: [u64; 16] = [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0];
    for at in 0..window.len().saturating_sub(4) {
        if window[at] != 0xff || window[at + 1] & 0xe0 != 0xe0 {
            continue;
        }
        let version = (window[at + 1] >> 3) & 3; // 3 = MPEG-1, 2 = MPEG-2, 0 = MPEG-2.5
        let layer = (window[at + 1] >> 1) & 3; // 1 = Layer III, 2 = II, 3 = I
        let bitrate_index = (window[at + 2] >> 4) as usize;
        let rate_index = ((window[at + 2] >> 2) & 3) as usize;
        if version == 1 || layer == 0 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
            continue;
        }
        let rate = [44_100u64, 48_000, 32_000][rate_index] >> if version == 3 { 0 } else if version == 2 { 1 } else { 2 };
        let bitrate = match (version == 3, layer) {
            (true, 1) => BITRATES_V1_L3[bitrate_index],
            (true, 2) => BITRATES_V1_L2[bitrate_index],
            (true, _) => BITRATES_V1_L1[bitrate_index],
            (false, 3) => BITRATES_V1_L1[bitrate_index] / 2,
            (false, _) => BITRATES_V2_L3[bitrate_index],
        };
        if bitrate == 0 {
            continue;
        }
        let samples_per_frame: u64 = match (layer, version == 3) {
            (3, _) => 384,
            (2, _) => 1152,
            (_, true) => 1152,
            (_, false) => 576,
        };
        // A Xing/Info header in the first frame states the frame count of a VBR file.
        let mono = (window[at + 3] >> 6) == 3;
        let side_info = match (version == 3, mono) {
            (true, false) => 32,
            (true, true) | (false, false) => 17,
            (false, true) => 9,
        };
        let tag = at + 4 + side_info;
        if layer == 1 && window.len() >= tag + 12 && (&window[tag..tag + 4] == b"Xing" || &window[tag..tag + 4] == b"Info") && window[tag + 7] & 1 != 0 {
            let frames = be32(&window[tag + 8..tag + 12]) * samples_per_frame;
            if frames > 0 {
                return Some(AudioInfo { duration: frames as f64 / rate as f64, frames: Some(frames), sample_rate: rate as f64 });
            }
        }
        let audio_bytes = size.saturating_sub(start + at as u64);
        return Some(AudioInfo { duration: audio_bytes as f64 * 8.0 / (bitrate as f64 * 1000.0), frames: None, sample_rate: rate as f64 });
    }
    None
}

/// Test fixtures: small valid audio files.
#[doc(hidden)]
pub mod fixtures {
    /// A 16-bit mono PCM WAV of `seconds` at 8 kHz, silent.
    pub fn wav(seconds: f64) -> Vec<u8> {
        wav_with(seconds, 8000, 1)
    }

    /// A 16-bit PCM WAV, silent.
    pub fn wav_with(seconds: f64, rate: u32, channels: u16) -> Vec<u8> {
        let frames = (seconds * f64::from(rate)).round() as u32;
        let block = u32::from(channels) * 2;
        let bytes = frames * block;
        let mut data = Vec::with_capacity(44 + bytes as usize);
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&(36 + bytes).to_le_bytes());
        data.extend_from_slice(b"WAVEfmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&channels.to_le_bytes());
        data.extend_from_slice(&rate.to_le_bytes());
        data.extend_from_slice(&(rate * block).to_le_bytes());
        data.extend_from_slice(&(block as u16).to_le_bytes());
        data.extend_from_slice(&16u16.to_le_bytes());
        data.extend_from_slice(b"data");
        data.extend_from_slice(&bytes.to_le_bytes());
        data.resize(44 + bytes as usize, 0);
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(name: &str, bytes: &[u8]) -> Option<AudioInfo> {
        let folder = std::env::temp_dir().join(format!("st-library-probe-{}-{}", std::process::id(), crate::util::unique_id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        std::fs::write(&path, bytes).unwrap();
        let info = audio_info(&path);
        let _ = std::fs::remove_dir_all(&folder);
        info
    }

    #[test]
    fn wav_headers() {
        let info = probe("a.wav", &fixtures::wav(0.25)).unwrap();
        assert_eq!((info.frames, info.sample_rate), (Some(2000), 8000.0));
        assert!((info.duration - 0.25).abs() < 1e-9);
        let stereo = probe("b.wav", &fixtures::wav_with(1.5, 44_100, 2)).unwrap();
        assert_eq!(stereo.frames, Some(66_150));
        // A data chunk that claims more than the file holds is clamped to the file.
        let mut truncated = fixtures::wav(1.0);
        truncated.truncate(44 + 8000);
        assert_eq!(probe("c.wav", &truncated).unwrap().frames, Some(4000));
        assert_eq!(probe("d.wav", &[1, 2, 3]), None);
        assert_eq!(probe("e.wav", b"RIFF\x04\0\0\0WAVE"), None);
    }

    #[test]
    fn aiff_flac_caf_headers() {
        let mut aiff = Vec::new();
        aiff.extend_from_slice(b"FORM\0\0\0\x2eAIFFCOMM\0\0\0\x12");
        aiff.extend_from_slice(&[0, 2]); // channels
        aiff.extend_from_slice(&88_200u32.to_be_bytes()); // frames
        aiff.extend_from_slice(&[0, 16]);
        aiff.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]); // 44100 as 80-bit extended
        aiff.extend_from_slice(b"SSND\0\0\0\x08\0\0\0\0\0\0\0\0");
        let info = probe("a.aif", &aiff).unwrap();
        assert_eq!((info.frames, info.sample_rate), (Some(88_200), 44_100.0));
        assert!((info.duration - 2.0).abs() < 1e-9);

        let mut flac = b"fLaC".to_vec();
        flac.extend_from_slice(&[0x80, 0, 0, 34]);
        let mut stream = [0u8; 34];
        let packed: u64 = (48_000u64 << 44) | (1 << 41) | (15 << 36) | 96_000;
        stream[10..18].copy_from_slice(&packed.to_be_bytes());
        flac.extend_from_slice(&stream);
        let info = probe("a.flac", &flac).unwrap();
        assert_eq!((info.frames, info.sample_rate, info.duration), (Some(96_000), 48_000.0, 2.0));

        let mut caf = b"caff\0\x01\0\0desc".to_vec();
        caf.extend_from_slice(&32u64.to_be_bytes());
        caf.extend_from_slice(&44_100f64.to_bits().to_be_bytes());
        caf.extend_from_slice(b"lpcm");
        caf.extend_from_slice(&0u32.to_be_bytes());
        caf.extend_from_slice(&4u32.to_be_bytes()); // bytes per packet
        caf.extend_from_slice(&1u32.to_be_bytes()); // frames per packet
        caf.extend_from_slice(&2u32.to_be_bytes());
        caf.extend_from_slice(&16u32.to_be_bytes());
        caf.extend_from_slice(b"data");
        caf.extend_from_slice(&(4u64 + 44_100 * 4).to_be_bytes());
        caf.extend_from_slice(&[0u8; 4]);
        caf.resize(caf.len() + 44_100 * 4, 0);
        let info = probe("a.caf", &caf).unwrap();
        assert_eq!((info.frames, info.duration), (Some(44_100), 1.0));
    }

    #[test]
    fn ogg_mp4_mp3_headers() {
        let page = |granule: u64, packet: &[u8]| -> Vec<u8> {
            let mut page = b"OggS\0\0".to_vec();
            page.extend_from_slice(&granule.to_le_bytes());
            page.extend_from_slice(&[0u8; 12]);
            page.push(1);
            page.push(packet.len() as u8);
            page.extend_from_slice(packet);
            page
        };
        let mut identification = b"\x01vorbis".to_vec();
        identification.extend_from_slice(&0u32.to_le_bytes());
        identification.push(2);
        identification.extend_from_slice(&44_100u32.to_le_bytes());
        identification.extend_from_slice(&[0u8; 14]);
        let mut ogg = page(0, &identification);
        ogg.extend_from_slice(&page(22_050, &[0u8; 40]));
        let info = probe("a.ogg", &ogg).unwrap();
        assert_eq!((info.frames, info.duration), (Some(22_050), 0.5));

        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&16u32.to_be_bytes());
        mp4.extend_from_slice(b"ftypM4A \0\0\0\0");
        let mut mvhd = vec![0u8; 20];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&2500u32.to_be_bytes());
        mp4.extend_from_slice(&(8 + 8 + 20u32).to_be_bytes());
        mp4.extend_from_slice(b"moov");
        mp4.extend_from_slice(&(8 + 20u32).to_be_bytes());
        mp4.extend_from_slice(b"mvhd");
        mp4.extend_from_slice(&mvhd);
        assert_eq!(probe("a.m4a", &mp4).unwrap().duration, 2.5);

        // MPEG-1 Layer III, 128 kbit/s, 44.1 kHz, constant bit rate: 16000 bytes per second.
        let mut mp3 = vec![0xff, 0xfb, 0x90, 0x00];
        mp3.resize(32_000, 0);
        let info = probe("a.mp3", &mp3).unwrap();
        assert!((info.duration - 2.0).abs() < 1e-9 && info.frames.is_none());
        assert_eq!(probe("b.mp3", &[0u8; 64]), None);
    }
}

//! Every supported container and codec is decoded from a real file (made by ffmpeg, see
//! tests/fixtures/generate.sh) and compared against the known signal:
//! left 0.5*sin(2*pi*440*t), right 0.25*sin(2*pi*1000*t); mono files hold the left channel.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use st_samples::{decode_file, decode_planar, probe_file, AudioFileReader, DecodeOptions, Operation, ResampleQuality, SampleError};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn reference(channel: usize, frame: usize, rate: f64) -> f64 {
    let t = frame as f64 / rate;
    if channel == 0 {
        0.5 * (2.0 * PI * 440.0 * t).sin()
    } else {
        0.25 * (2.0 * PI * 1000.0 * t).sin()
    }
}

struct Expectation {
    file: &'static str,
    container: &'static str,
    codec: &'static str,
    rate: u32,
    channels: usize,
    /// Exact frame count of the source signal.
    frames: usize,
    /// Lossless: largest allowed sample error. Lossy: `None`.
    exact_within: Option<f64>,
    /// Lossy: smallest signal-to-error ratio in dB over the aligned interior.
    min_snr_db: f64,
    /// Lossy: how many frames the decoded length may exceed the source (encoder padding that the
    /// file does not declare).
    extra_frames: usize,
    /// Frames by which the decoded audio is known to lag the source (encoder delay that the file
    /// does not declare).
    lag: usize,
}

const fn lossless(file: &'static str, container: &'static str, codec: &'static str, rate: u32, channels: usize, frames: usize, tolerance: f64) -> Expectation {
    Expectation { file, container, codec, rate, channels, frames, exact_within: Some(tolerance), min_snr_db: 0.0, extra_frames: 0, lag: 0 }
}

const fn lossy(file: &'static str, container: &'static str, codec: &'static str, rate: u32, channels: usize, frames: usize, min_snr_db: f64, extra_frames: usize, lag: usize) -> Expectation {
    Expectation { file, container, codec, rate, channels, frames, exact_within: None, min_snr_db, extra_frames, lag }
}

const LSB16: f64 = 1.0 / 32_768.0;
const LSB24: f64 = 1.0 / 8_388_608.0;

const FIXTURES: &[Expectation] = &[
    lossless("wav_s16.wav", "wave", "pcm_s16le", 44_100, 2, 4410, LSB16),
    lossless("wav_s24_extensible.wav", "wave", "pcm_s24le", 44_100, 2, 4410, LSB24),
    lossless("wav_s32_48k.wav", "wave", "pcm_s32le", 48_000, 2, 4800, 1e-7),
    lossless("wav_f32.wav", "wave", "pcm_f32le", 44_100, 2, 4410, 1e-7),
    lossless("wav_u8_mono.wav", "wave", "pcm_u8", 44_100, 1, 4410, 1.0 / 128.0),
    lossless("aiff_s16.aiff", "aiff", "pcm_s16be", 44_100, 2, 4410, LSB16),
    lossless("aiff_s24_96k.aiff", "aiff", "pcm_s24be", 96_000, 2, 4800, LSB24),
    lossless("aifc_sowt.aifc", "aiff", "pcm_s16le", 44_100, 2, 4410, LSB16),
    lossless("aifc_fl32_mono.aifc", "aiff", "pcm_f32be", 44_100, 1, 4410, 1e-7),
    lossless("caf_s16.caf", "caf", "pcm_s16be", 44_100, 2, 4410, LSB16),
    lossless("caf_f32_48k.caf", "caf", "pcm_f32le", 48_000, 2, 4800, 1e-7),
    lossless("caf_alac.caf", "caf", "alac", 44_100, 2, 4410, LSB16),
    lossless("flac_16.flac", "flac", "flac", 44_100, 2, 4410, LSB16),
    lossless("flac_24_96k.flac", "flac", "flac", 96_000, 2, 4800, LSB24),
    lossless("m4a_alac.m4a", "isomp4", "alac", 44_100, 2, 4410, LSB16),
    // Companded and ADPCM encodings are lossy by design.
    lossy("wav_ulaw_mono_22k.wav", "wave", "pcm_mulaw", 22_050, 1, 2205, 30.0, 0, 0),
    lossy("wav_alaw_mono_22k.wav", "wave", "pcm_alaw", 22_050, 1, 2205, 30.0, 0, 0),
    lossy("wav_adpcm_ima.wav", "wave", "adpcm_ima_wav", 44_100, 2, 22_050, 30.0, 2048, 0),
    lossy("wav_adpcm_ms.wav", "wave", "adpcm_ms", 44_100, 2, 22_050, 30.0, 2048, 0),
    // The thresholds sit 1.5 dB under what ffmpeg's own decoder gives for the same files
    // (ffmpeg: AAC 24.8/45.7 dB, mono AAC 32.6, MP3 30.5/30.5, VBR MP3 44.5, MP2 44.3/31.2,
    // Vorbis 41.2/41.4): the codecs, not the decoders, set these numbers.
    lossy("m4a_aac.m4a", "isomp4", "aac", 44_100, 2, 22_050, 23.3, 0, 0),
    lossy("m4a_aac_mono_48k.m4a", "isomp4", "aac", 48_000, 1, 24_000, 31.1, 0, 0),
    // Raw ADTS carries no delay information: the encoder's 1024 priming frames stay.
    lossy("aac_adts.aac", "aac", "aac", 44_100, 2, 22_050, 23.3, 2048, 1024),
    lossy("mp3_cbr.mp3", "mp3", "mp3", 44_100, 2, 22_050, 29.0, 0, 0),
    lossy("mp3_vbr_mono_48k.mp3", "mp3", "mp3", 48_000, 1, 24_000, 43.0, 0, 0),
    // MPEG layer 2 has no gapless header either: 481 frames of codec delay stay.
    lossy("mp2.mp2", "mp2", "mp2", 44_100, 2, 22_050, 29.7, 1152 * 2, 481),
    lossy("ogg_vorbis.ogg", "ogg", "vorbis", 44_100, 2, 22_050, 39.7, 0, 0),
];

/// Signal-to-error ratio in dB of `decoded` (lagging the source by `lag` frames), ignoring
/// `margin` frames at each end where lossy codecs ring.
fn snr_db(decoded: &[f32], channel: usize, rate: f64, lag: usize, frames: usize, margin: usize) -> f64 {
    let (mut signal, mut error) = (0.0, 0.0);
    for frame in margin..frames.saturating_sub(margin) {
        let Some(&value) = decoded.get(frame + lag) else { break };
        let expected = reference(channel, frame, rate);
        signal += expected * expected;
        error += (value as f64 - expected).powi(2);
    }
    10.0 * (signal / error.max(1e-30)).log10()
}

#[test]
fn every_supported_format_decodes_to_the_known_signal() {
    for expected in FIXTURES {
        let path = fixture(expected.file);
        let spec = probe_file(&path).unwrap_or_else(|e| panic!("{}: {e}", expected.file));
        assert_eq!((spec.container.as_str(), spec.codec.as_str()), (expected.container, expected.codec), "{}", expected.file);
        assert_eq!((spec.sample_rate, spec.channels), (expected.rate, expected.channels), "{}", expected.file);
        let sample = decode_file(expected.file, &path, &mut Operation::new()).unwrap_or_else(|e| panic!("{}: {e}", expected.file));
        assert_eq!(sample.sample_rate, expected.rate as f64);
        assert_eq!(sample.original_channels, expected.channels, "{}", expected.file);
        assert_eq!(sample.left.len(), sample.right.len());
        assert!(sample.left.iter().chain(&sample.right).all(|v| v.is_finite()));
        let rate = expected.rate as f64;
        let decoded = sample.left.len();
        if let Some(tolerance) = expected.exact_within {
            assert_eq!(decoded, expected.frames, "{} length", expected.file);
            if expected.file == "caf_alac.caf" {
                // ffmpeg's CAF muxer declares whole ALAC packets; the codec knows the real length.
                assert!(spec.frames >= Some(expected.frames as u64));
            } else {
                assert_eq!(spec.frames, Some(expected.frames as u64), "{} declared length", expected.file);
            }
            let mut worst = 0f64;
            for frame in 0..decoded {
                worst = worst.max((sample.left[frame] as f64 - reference(0, frame, rate)).abs());
                let right = if expected.channels == 1 { reference(0, frame, rate) } else { reference(1, frame, rate) };
                worst = worst.max((sample.right[frame] as f64 - right).abs());
            }
            println!("MEASURE decode {:<24} {:>7}/{:<14} {} Hz {} ch {} frames, max error {:.3e}", expected.file, spec.container, spec.codec, expected.rate, expected.channels, decoded, worst);
            assert!(worst <= tolerance * 1.01, "{}: max error {worst} exceeds {tolerance}", expected.file);
        } else {
            assert!(
                decoded + 128 >= expected.frames && decoded <= expected.frames + expected.extra_frames,
                "{}: decoded {decoded} frames for a source of {} (+{} allowed)",
                expected.file,
                expected.frames,
                expected.extra_frames
            );
            let left = snr_db(&sample.left, 0, rate, expected.lag, expected.frames, 2048.min(expected.frames / 8));
            let right = snr_db(&sample.right, if expected.channels == 1 { 0 } else { 1 }, rate, expected.lag, expected.frames, 2048.min(expected.frames / 8));
            println!(
                "MEASURE decode {:<24} {:>7}/{:<14} {} Hz {} ch {} frames (source {}), SNR L {:.1} dB R {:.1} dB at lag {}",
                expected.file, spec.container, spec.codec, expected.rate, expected.channels, decoded, expected.frames, left, right, expected.lag
            );
            assert!(left.min(right) >= expected.min_snr_db, "{}: SNR {left:.1}/{right:.1} dB below {}", expected.file, expected.min_snr_db);
        }
    }
}

#[test]
fn mono_is_duplicated_and_blocks_stream() {
    let path = fixture("wav_u8_mono.wav");
    let sample = decode_file("mono", &path, &mut Operation::new()).unwrap();
    assert_eq!(sample.left, sample.right);
    assert_eq!(sample.info().channels, 1);
    let mut reader = AudioFileReader::open(&fixture("mp3_cbr.mp3")).unwrap();
    let (mut total, mut blocks) = (0, 0);
    while let Some(block) = reader.next_block().unwrap() {
        assert_eq!(block.len(), 2);
        assert!(!block[0].is_empty() && block[0].len() == block[1].len());
        total += block[0].len();
        blocks += 1;
    }
    assert!(blocks > 5 && total as u64 == reader.position() && total == 22_050);
    assert!(reader.next_block().unwrap().is_none(), "the end is sticky");
}

#[test]
fn decode_planar_converts_rate_and_channel_layout() {
    let native = decode_planar(&fixture("flac_16.flac"), &DecodeOptions::default(), &mut Operation::new()).unwrap();
    assert_eq!((native.sample_rate, native.channels.len(), native.frames()), (44_100, 2, 4410));
    let options = DecodeOptions { target_rate: Some(48_000), quality: ResampleQuality::Mastering, stereo: true, max_frames: None };
    let converted = decode_planar(&fixture("wav_u8_mono.wav"), &options, &mut Operation::new()).unwrap();
    assert_eq!((converted.sample_rate, converted.channels.len(), converted.frames()), (48_000, 2, 4800));
    assert_eq!(converted.channels[0], converted.channels[1]);
    let limited = decode_planar(&fixture("mp3_cbr.mp3"), &DecodeOptions { max_frames: Some(1000), ..options }, &mut Operation::new()).unwrap();
    assert_eq!(limited.frames(), 1000);
    // The converted lossless file is still the known signal, at the new rate.
    let converted = decode_planar(&fixture("flac_16.flac"), &options, &mut Operation::new()).unwrap();
    let worst = (600..4200).map(|frame| (converted.channels[0][frame] as f64 - reference(0, frame, 48_000.0)).abs()).fold(0f64, f64::max);
    assert!(worst < 3.0 * LSB16, "44.1 -> 48 kHz conversion error {worst}");
}

#[test]
fn progress_cancellation_and_failures() {
    let path = fixture("ogg_vorbis.ogg");
    let mut seen = Vec::new();
    let mut callback = |progress: st_samples::Progress| seen.push(progress);
    decode_file("x", &path, &mut Operation::new().with_progress(&mut callback)).unwrap();
    assert!(seen.len() > 3 && seen.windows(2).all(|pair| pair[0].done <= pair[1].done));
    assert_eq!(seen.last().unwrap().done, 22_050);
    assert_eq!(seen.last().unwrap().fraction(), Some(1.0));

    let token = st_samples::CancelToken::new();
    token.cancel();
    assert_eq!(decode_file("x", &path, &mut Operation::new().with_cancel(&token)).unwrap_err(), SampleError::Cancelled);

    let missing = decode_file("/music/Missing Kick.wav", Path::new("/nonexistent-st-samples/Missing Kick.wav"), &mut Operation::new()).unwrap_err();
    assert!(matches!(missing, SampleError::NotFound(_)));
    assert_eq!(missing.to_string(), "Sample file not found: Missing Kick.wav. Use Relink missing project media to locate it.");

    let dir = std::env::temp_dir().join(format!("st-samples-decode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join("notes.wav");
    std::fs::write(&text, "this is not audio, whatever the extension says\n".repeat(50)).unwrap();
    let error = decode_file("notes.wav", &text, &mut Operation::new()).unwrap_err();
    assert!(matches!(error, SampleError::Unsupported(_)) && error.to_string().contains("notes.wav"), "{error}");
    let empty = dir.join("empty.wav");
    std::fs::write(&empty, b"").unwrap();
    assert!(decode_file("empty.wav", &empty, &mut Operation::new()).is_err());
    assert!(matches!(decode_file("dir", &dir, &mut Operation::new()).unwrap_err(), SampleError::Unsupported(_)));
    // A truncated file keeps what is there.
    let cut = dir.join("cut.wav");
    let bytes = std::fs::read(fixture("wav_s16.wav")).unwrap();
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let partial = decode_file("cut.wav", &cut, &mut Operation::new()).unwrap();
    assert!(partial.left.len() > 2000 && partial.left.len() < 4410, "{}", partial.left.len());
    std::fs::remove_dir_all(&dir).ok();
}

//! Speed measurements. Not run by default: they need `ffmpeg` to make five-minute files and
//! only mean something in an optimised build.
//!
//! ```text
//! cargo test --release -p st-samples --test performance -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use st_samples::preview::PreviewCache;
use st_samples::resample::resample;
use st_samples::{analyze_slices, decode_file, CancelToken, Operation, ResampleQuality, SliceRequest};

const SECONDS: u32 = 300;

fn scratch() -> PathBuf {
    let path = std::env::temp_dir().join(format!("st-samples-performance-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Two tones and noise per channel: enough going on that no codec gets an easy ride.
fn encode(target: &Path, rate: u32, arguments: &[&str]) -> bool {
    let source = format!("aevalsrc=0.3*sin(2*PI*220*t)+0.2*sin(2*PI*3300*t)+0.1*random(0)|0.3*sin(2*PI*330*t)+0.2*sin(2*PI*5500*t)+0.1*random(1):s={rate}:d={SECONDS}");
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &source])
        .args(arguments)
        .arg(target)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, f64) {
    let began = Instant::now();
    let value = work();
    (value, began.elapsed().as_secs_f64())
}

#[test]
#[ignore = "needs ffmpeg and a release build"]
fn decode_speed_for_a_five_minute_stereo_file_per_format() {
    let folder = scratch();
    let formats: &[(&str, &[&str])] = &[
        ("wav_s16.wav", &["-c:a", "pcm_s16le"]),
        ("wav_s24.wav", &["-c:a", "pcm_s24le"]),
        ("wav_f32.wav", &["-c:a", "pcm_f32le"]),
        ("aiff_s24.aiff", &["-c:a", "pcm_s24be"]),
        ("caf_f32.caf", &["-c:a", "pcm_f32le"]),
        ("flac_16.flac", &["-c:a", "flac", "-sample_fmt", "s16"]),
        ("flac_24.flac", &["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"]),
        ("alac.m4a", &["-c:a", "alac"]),
        ("alac.caf", &["-c:a", "alac"]),
        ("aac_192k.m4a", &["-c:a", "aac", "-b:a", "192k"]),
        ("mp3_192k.mp3", &["-c:a", "libmp3lame", "-b:a", "192k"]),
        ("mp3_v2.mp3", &["-c:a", "libmp3lame", "-q:a", "2"]),
        ("mp2_192k.mp2", &["-c:a", "mp2", "-b:a", "192k"]),
        ("vorbis_q6.ogg", &["-c:a", "libvorbis", "-q:a", "6"]),
        ("adpcm_ima.wav", &["-c:a", "adpcm_ima_wav"]),
    ];
    for (name, arguments) in formats {
        let path = folder.join(name);
        if !encode(&path, 44_100, arguments) {
            println!("MEASURE decode 5 min {name}: skipped (ffmpeg could not encode it)");
            continue;
        }
        let megabytes = std::fs::metadata(&path).unwrap().len() as f64 / 1e6;
        // Best of three: the first run also pays for reading the file into the page cache.
        let mut best = f64::MAX;
        let mut frames = 0;
        for _ in 0..3 {
            let (sample, seconds) = timed(|| decode_file(name, &path, &mut Operation::new()).unwrap());
            frames = sample.left.len();
            best = best.min(seconds);
        }
        let audio_seconds = frames as f64 / 44_100.0;
        println!("MEASURE decode 5 min stereo 44.1 kHz {name:<14} {megabytes:>6.1} MB: {:>7.1} ms = {:>6.0}x real time ({frames} frames)", best * 1000.0, audio_seconds / best);
        assert!((audio_seconds - SECONDS as f64).abs() < 0.2, "{name}: {audio_seconds} s");
        std::fs::remove_file(&path).ok();
    }
    std::fs::remove_dir_all(&folder).ok();
}

#[test]
#[ignore = "release build only"]
fn resample_stretch_slice_and_preview_speed() {
    let noise = |frames: usize, seed: u32| -> Vec<f32> {
        let mut state = seed;
        (0..frames)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    };
    for (from, to) in [(44_100u32, 48_000u32), (48_000, 44_100), (48_000, 96_000), (96_000, 44_100)] {
        let frames = from as usize * SECONDS as usize;
        let input = [noise(frames, 1), noise(frames, 2)];
        for quality in [ResampleQuality::Preview, ResampleQuality::Standard, ResampleQuality::Mastering] {
            let (output, seconds) = timed(|| resample(&input, from, to, quality).unwrap());
            println!(
                "MEASURE resample 5 min stereo {from}->{to} {quality:?}: {:>7.0} ms = {:>5.0}x real time ({} taps)",
                seconds * 1000.0,
                SECONDS as f64 / seconds,
                st_samples::Resampler::new(from, to, 2, quality).unwrap().filter_length()
            );
            assert_eq!(output[0].len() as u64, (frames as u64 * to as u64).div_ceil(from as u64));
        }
    }

    let frames = 48_000 * 60;
    let sample = st_samples::DecodedSample::new("noise", 48_000.0, noise(frames, 3), noise(frames, 4), 2);
    let (_, seconds) = timed(|| st_samples::stretch::stretch(&sample, 1.5, &mut Operation::new()).unwrap());
    println!("MEASURE stretch 1 min stereo 48 kHz to 150 %: {:.0} ms = {:.0}x real time", seconds * 1000.0, 60.0 / seconds);
    let (analysis, seconds) = timed(|| analyze_slices(&sample, &SliceRequest::default(), &mut Operation::new()).unwrap());
    println!("MEASURE slice analysis of 1 min stereo 48 kHz: {:.1} ms ({} windows)", seconds * 1000.0, analysis.analysis_windows);
    let (_, seconds) = timed(|| sample.info());
    println!("MEASURE info with 512 peaks of 1 min stereo: {:.2} ms", seconds * 1000.0);

    // Preview: how long until a file can start playing.
    let folder = scratch();
    for (name, arguments) in [("preview.wav", &["-c:a", "pcm_s16le"][..]), ("preview.mp3", &["-c:a", "libmp3lame", "-b:a", "192k"][..]), ("preview.flac", &["-c:a", "flac"][..])] {
        let path = folder.join(name);
        if !encode(&path, 44_100, arguments) {
            continue;
        }
        let cache = PreviewCache::default();
        let path = path.to_string_lossy().into_owned();
        let (head, cold) = timed(|| cache.head(&path, 48_000, &CancelToken::new()).unwrap());
        let (_, warm) = timed(|| cache.head(&path, 48_000, &CancelToken::new()).unwrap());
        println!(
            "MEASURE preview {name}: 4 s head of a 5 min file at 48 kHz in {:.1} ms, cached {:.4} ms ({} frames, complete {})",
            cold * 1000.0,
            warm * 1000.0,
            head.pcm.frames(),
            head.pcm.is_complete()
        );
    }
    std::fs::remove_dir_all(&folder).ok();
}

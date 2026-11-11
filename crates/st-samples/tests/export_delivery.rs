//! Delivery of a float render as a WAV file: exact lengths, sample-rate conversion quality,
//! encodings, cancellation and cleanup. Files only; no devices, engine or plug-ins.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use st_samples::export::delivery::{output_frames, preflight};
use st_samples::export::{DeliveryJob, DeliverySettings, ExportEncoding, RenderFileWriter};
use st_samples::wav::{read_header, Dither};
use st_samples::{decode_file, Operation};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("st-samples-delivery-{}-{name}-{}", std::process::id(), st_samples::paths::unique_suffix()));
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn write_render(path: &Path, left: &[f32], right: &[f32], rate: u32) {
    let mut writer = RenderFileWriter::create(path, rate, 2).unwrap();
    writer.write_planar(&[left, right]).unwrap();
    writer.finalize().unwrap();
}

fn read(path: &Path) -> [Vec<f32>; 2] {
    let sample = decode_file("delivered", path, &mut Operation::new()).unwrap();
    [sample.left, sample.right]
}

fn settings(encoding: ExportEncoding, rate: u32) -> DeliverySettings {
    DeliverySettings::new(encoding, rate).unwrap()
}

/// Runs a job to completion in small steps, checks the invariants along the way and the
/// written header, and returns the delivered audio.
fn converted(source: &Path, destination: &Path, frames: usize, settings: DeliverySettings, block: usize) -> [Vec<f32>; 2] {
    let mut job = DeliveryJob::new(source, frames as u64, destination, settings, block).unwrap();
    let mut calls = 0;
    while job.result().is_none() {
        let progress = job.advance(3).unwrap();
        calls += 1;
        assert!(calls < 1_000_000, "converter never terminated");
        assert!(progress.output_frames <= progress.total_output_frames && progress.input_frames <= frames as u64, "invalid progress counters");
        assert!(!destination.exists(), "an incomplete job became visible at its destination");
    }
    let result = job.publish().unwrap();
    assert!(job.is_published());
    let (rate, encoding) = (settings.sample_rate(), settings.encoding());
    let expected = output_frames(frames as u64, 48_000, rate).unwrap();
    let header = read_header(destination).expect("a WAV header");
    assert_eq!((header.frames, header.sample_rate, header.channels), (expected, rate, 2), "actual WAV length, rate and channels");
    assert_eq!((header.bits_per_sample as u32, header.is_float()), (encoding.bit_depth(), encoding.is_float()), "actual WAV encoding");
    assert_eq!((result.output_frames, result.input_frames), (expected, frames as u64));
    assert_eq!(result.metadata.sample_rate, rate);
    let decoded = read(destination);
    assert_eq!(decoded[0].len() as u64, expected, "exact ceiling frame count");
    assert!(decoded.iter().flatten().all(|v| v.is_finite()));
    decoded
}

fn max_difference(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2]) -> f64 {
    a.iter().flatten().zip(b.iter().flatten()).fold(0f64, |m, (x, y)| m.max((*x as f64 - *y as f64).abs()))
}

#[test]
fn rational_endpoints_overflow_and_riff_admission() {
    for (input, rate, expected) in [(0u64, 44_100u32, 0u64), (1, 44_100, 1), (159, 44_100, 147), (160, 44_100, 147), (161, 44_100, 148), (6007, 96_000, 12_014), (i64::MAX as u64, 48_000, i64::MAX as u64)] {
        assert_eq!(output_frames(input, 48_000, rate).unwrap(), expected, "{input} frames at {rate}");
    }
    assert!(output_frames(i64::MAX as u64, 48_000, 96_000).is_err(), "overflow");
    assert!(DeliverySettings::new(ExportEncoding::Pcm24, 88_200).is_err(), "unknown rate");
    assert!(preflight(1_000_000_000, &settings(ExportEncoding::Float32, 96_000)).is_err(), "RIFF bound");
    assert!(preflight(0, &settings(ExportEncoding::Pcm16, 48_000)).is_err(), "empty source");
    assert_eq!(preflight(480_000, &settings(ExportEncoding::Pcm16, 44_100)).unwrap(), 441_000);
    // A musical part and its tail must be split at absolute end points.
    let (musical, total) = (80u64, 160u64);
    let tail = output_frames(total, 48_000, 44_100).unwrap() - output_frames(musical, 48_000, 44_100).unwrap();
    assert!(tail == 73 && tail != output_frames(total - musical, 48_000, 44_100).unwrap());
    // Another render rate works the same way.
    assert_eq!(output_frames(44_100, 44_100, 48_000).unwrap(), 48_000);
}

#[test]
fn all_rates_and_encodings_for_tiny_and_odd_lengths() {
    let scratch = Scratch::new("tiny");
    for count in [1usize, 2, 3, 159, 160, 161, 4095, 4097] {
        let source = scratch.path(&format!("tiny-{count}.caf"));
        let left: Vec<f32> = (0..count).map(|i| (0.25 + 0.1 * (i as f64 * 0.1).sin()) as f32).collect();
        let right: Vec<f32> = left.iter().map(|v| -v * 0.7).collect();
        write_render(&source, &left, &right, 48_000);
        for rate in [44_100, 48_000, 96_000] {
            for encoding in ExportEncoding::ALL {
                let destination = scratch.path(&format!("tiny-{count}-{rate}-{}.wav", encoding.name()));
                converted(&source, &destination, count, settings(encoding, rate), 37);
            }
        }
    }
}

#[test]
fn conversion_is_block_invariant_keeps_float_overshoot_and_bypasses_at_the_render_rate() {
    let scratch = Scratch::new("blocks");
    let count = 16_011;
    let source = scratch.path("source.caf");
    let left: Vec<f32> = (0..count).map(|i| (1.21 * (2.0 * PI * 997.0 * i as f64 / 48_000.0).sin() + 0.07 * (i as f64 * 0.3).cos()) as f32).collect();
    let right: Vec<f32> = (0..count).map(|i| (0.33 * (2.0 * PI * 311.0 * i as f64 / 48_000.0 + 0.2).sin()) as f32).collect();
    write_render(&source, &left, &right, 48_000);
    let original = std::fs::read(&source).unwrap();
    for rate in [44_100, 48_000, 96_000] {
        let float = settings(ExportEncoding::Float32, rate);
        let first = converted(&source, &scratch.path(&format!("block-{rate}-a.wav")), count, float, 4096);
        let second = converted(&source, &scratch.path(&format!("block-{rate}-b.wav")), count, float, 37);
        let third = converted(&source, &scratch.path(&format!("block-{rate}-c.wav")), count, float, 8192);
        assert!(first == second && first == third, "block size changed the conversion at {rate} Hz");
        // Whole files, not only the samples, are identical.
        assert_eq!(std::fs::read(scratch.path(&format!("block-{rate}-a.wav"))).unwrap(), std::fs::read(scratch.path(&format!("block-{rate}-b.wav"))).unwrap());
        let peak = first[0].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 1.1, "float overshoot was clipped or normalised at {rate} Hz: {peak}");
        if rate == 48_000 {
            let same = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
            assert!(same(&left, &first[0]) && same(&right, &first[1]), "48 kHz float delivery is not bit-exact");
        }
    }
    assert_eq!(std::fs::read(&source).unwrap(), original, "the intermediate render was modified");
}

#[test]
fn impulses_stay_at_their_time_in_both_channels() {
    let scratch = Scratch::new("impulse");
    let source = scratch.path("impulse.caf");
    let (mut left, mut right) = (vec![0f32; 9600], vec![0f32; 9600]);
    left[3200] = 1.0;
    right[4800] = 0.5;
    write_render(&source, &left, &right, 48_000);
    for rate in [44_100usize, 48_000, 96_000] {
        let samples = converted(&source, &scratch.path(&format!("impulse-{rate}.wav")), 9600, settings(ExportEncoding::Float32, rate as u32), 4096);
        for (channel, position) in [(0, 3200usize), (1, 4800)] {
            let peak = (0..samples[channel].len()).max_by(|&a, &b| samples[channel][a].abs().total_cmp(&samples[channel][b].abs())).unwrap();
            assert_eq!(peak, position * rate / 48_000, "the converter shifted the impulse at {rate} Hz");
        }
    }
}

#[test]
fn passband_fidelity_alias_rejection_and_one_final_quantisation() {
    let scratch = Scratch::new("tones");
    let source = scratch.path("tones.caf");
    let count = 48_000;
    let low: Vec<f32> = (0..count).map(|i| (0.5 * (2.0 * PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32).collect();
    let high: Vec<f32> = (0..count).map(|i| (0.5 * (2.0 * PI * 23_000.0 * i as f64 / 48_000.0).sin()) as f32).collect();
    write_render(&source, &low, &high, 48_000);
    let reduced = converted(&source, &scratch.path("tones-44100.wav"), count, settings(ExportEncoding::Float32, 44_100), 4096);
    let interior = 2000..reduced[0].len() - 2000;
    let tone_error = (interior.clone().map(|i| (reduced[0][i] as f64 - 0.5 * (2.0 * PI * 1000.0 * i as f64 / 44_100.0).sin()).powi(2)).sum::<f64>() / interior.len() as f64).sqrt();
    let alias = (interior.clone().map(|i| (reduced[1][i] as f64).powi(2)).sum::<f64>() / interior.len() as f64).sqrt();
    println!(
        "MEASURE delivery 48k->44.1k: 1 kHz tone RMS error {tone_error:.2e} ({:.1} dB), 23 kHz alias RMS {alias:.2e} ({:.1} dB re the tone)",
        20.0 * (tone_error / (0.5 / 2f64.sqrt())).log10(),
        20.0 * (alias.max(1e-30) / (0.5 / 2f64.sqrt())).log10()
    );
    assert!(tone_error < 1e-6, "1 kHz phase/amplitude error {tone_error}");
    assert!(alias < 1e-6, "23 kHz anti-alias rejection: RMS {alias}");
    for encoding in [ExportEncoding::Pcm16, ExportEncoding::Pcm24] {
        let lsb = 1.0 / 2f64.powi(encoding.bit_depth() as i32 - 1);
        let plain = converted(&source, &scratch.path(&format!("tones-44100-{}.wav", encoding.name())), count, settings(encoding, 44_100), 4096);
        let error = max_difference(&reduced, &plain);
        assert!(error <= 0.5 * lsb + 1e-9, "{}: rounding error {error} exceeds half a step", encoding.name());
        let dithered = converted(&source, &scratch.path(&format!("tones-44100-{}-tpdf.wav", encoding.name())), count, settings(encoding, 44_100).with_dither(Dither::Tpdf), 4096);
        let error = max_difference(&reduced, &dithered);
        assert!(error <= 1.5 * lsb + 1e-9 && error > 0.5 * lsb, "{}: dithered error {error}", encoding.name());
        assert!(plain != dithered);
    }
}

#[test]
fn integer_and_float_content_round_trips_bit_exactly_without_dither() {
    let scratch = Scratch::new("roundtrip");
    let count = 20_000;
    let mut state = 0x2468_ACE1u32;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for encoding in ExportEncoding::ALL {
        // Audio that already has the target word length, including both extremes.
        let scale = 2f64.powi(encoding.bit_depth() as i32 - 1);
        let mut value = |index: usize| -> f32 {
            match encoding {
                ExportEncoding::Float32 => (random() as f64 / u32::MAX as f64 * 3.0 - 1.5) as f32,
                _ if index == 0 => -1.0,
                _ if index == 1 => ((scale - 1.0) / scale) as f32,
                _ => (((random() as f64 / u32::MAX as f64) * 2.0 * scale).floor().min(2.0 * scale - 1.0) - scale) as f32 / scale as f32,
            }
        };
        let left: Vec<f32> = (0..count).map(&mut value).collect();
        let right: Vec<f32> = (0..count).map(&mut value).collect();
        let source = scratch.path(&format!("{}.caf", encoding.name()));
        write_render(&source, &left, &right, 48_000);
        let first = scratch.path(&format!("{}-1.wav", encoding.name()));
        let delivered = converted(&source, &first, count, settings(encoding, 48_000), 4096);
        assert!(delivered[0] == left && delivered[1] == right, "{} delivery changed audio of its own word length", encoding.name());
        // Decode, render again, deliver again: the second file is byte-identical to the first.
        let again = scratch.path(&format!("{}-again.caf", encoding.name()));
        write_render(&again, &delivered[0], &delivered[1], 48_000);
        let second = scratch.path(&format!("{}-2.wav", encoding.name()));
        converted(&again, &second, count, settings(encoding, 48_000), 1000);
        assert_eq!(std::fs::read(&first).unwrap(), std::fs::read(&second).unwrap(), "{} files differ after a round trip", encoding.name());
    }
    // Integer encodings clip instead of wrapping.
    let source = scratch.path("hot.caf");
    write_render(&source, &[1.5, -1.5, 0.999_999], &[-3.0, 2.0, 0.0], 48_000);
    let clipped = converted(&source, &scratch.path("hot.wav"), 3, settings(ExportEncoding::Pcm16, 48_000), 4096);
    assert_eq!(clipped[0], vec![32_767.0 / 32_768.0, -1.0, 32_767.0 / 32_768.0]);
    assert_eq!(clipped[1], vec![-1.0, 32_767.0 / 32_768.0, 0.0]);
}

#[test]
fn cancellation_collisions_and_existing_files() {
    let scratch = Scratch::new("cancel");
    let count = 6000;
    let source = scratch.path("source.caf");
    let tone: Vec<f32> = (0..count).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    write_render(&source, &tone, &tone, 48_000);
    let cancelled = scratch.path("cancel.wav");
    let mut cancel = DeliveryJob::new(&source, count as u64, &cancelled, settings(ExportEncoding::Pcm24, 96_000), 37).unwrap();
    cancel.advance(1).unwrap();
    cancel.discard();
    assert!(!cancelled.exists(), "cancellation published an incomplete destination");
    assert!(cancel.advance(8).is_err(), "a cancelled job cannot continue");
    assert!(cancel.publish().is_err());

    let collision = scratch.path("collision.wav");
    let mut job = DeliveryJob::new(&source, count as u64, &collision, settings(ExportEncoding::Float32, 48_000), 4096).unwrap();
    assert!(job.publish().is_err(), "an unfinished job cannot publish");
    while job.result().is_none() {
        job.advance(8).unwrap();
    }
    std::fs::write(&collision, b"existing file must survive").unwrap();
    assert!(job.publish().is_err(), "late target collision");
    job.discard();
    assert_eq!(std::fs::read(&collision).unwrap(), b"existing file must survive", "a late collision damaged existing bytes");
    assert!(DeliveryJob::new(&source, count as u64, &collision, settings(ExportEncoding::Pcm16, 48_000), 4096).is_err(), "existing target");

    // A dropped job cleans up like a discarded one.
    let dropped = scratch.path("dropped.wav");
    {
        let mut job = DeliveryJob::new(&source, count as u64, &dropped, settings(ExportEncoding::Pcm16, 44_100), 512).unwrap();
        job.advance(2).unwrap();
    }
    assert!(!dropped.exists());
    let leftovers: Vec<_> = std::fs::read_dir(&scratch.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(".delivery-")).collect();
    assert!(leftovers.is_empty(), "private delivery folders leaked: {leftovers:?}");
}

#[test]
fn malformed_sources_and_non_finite_audio() {
    let scratch = Scratch::new("malformed");
    let count = 500;
    let source = scratch.path("source.caf");
    let tone: Vec<f32> = (0..count).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    write_render(&source, &tone, &tone, 48_000);
    let original = std::fs::read(&source).unwrap();
    let destination = scratch.path("out.wav");
    let pcm24 = settings(ExportEncoding::Pcm24, 48_000);
    assert!(DeliveryJob::new(&source, count as u64 + 1, &destination, pcm24, 4096).is_err(), "source extent mismatch");
    assert!(DeliveryJob::new(&source, count as u64, &destination, pcm24, 0).is_err(), "zero block size");
    assert!(DeliveryJob::new(&source, count as u64, &destination, pcm24, 65_537).is_err(), "oversized block");
    assert!(DeliveryJob::new(&source, count as u64, &source, pcm24, 4096).is_err(), "source and destination are the same file");
    assert!(DeliveryJob::new(&source, count as u64, &scratch.path("out.aiff"), pcm24, 4096).is_err(), "not a .wav destination");
    assert!(DeliveryJob::new(&source, count as u64, &scratch.path("missing/out.wav"), pcm24, 4096).is_err(), "missing folder");
    assert!(DeliveryJob::new(&scratch.path("absent.caf"), count as u64, &destination, pcm24, 4096).is_err(), "missing source");
    let wrong_rate = scratch.path("wrong-rate.caf");
    write_render(&wrong_rate, &[0.1, 0.2], &[0.1, 0.2], 44_100);
    assert!(DeliveryJob::new(&wrong_rate, 2, &destination, pcm24, 4096).is_err(), "a render at another rate");
    let mono = scratch.path("mono.caf");
    let mut writer = RenderFileWriter::create(&mono, 48_000, 1).unwrap();
    writer.write_planar(&[&[0.1f32, 0.2][..]]).unwrap();
    writer.finalize().unwrap();
    assert!(DeliveryJob::new(&mono, 2, &destination, pcm24, 4096).is_err(), "a mono render");
    let wav_named = scratch.path("render.wav");
    std::fs::copy(&source, &wav_named).unwrap();
    assert!(DeliveryJob::new(&wav_named, count as u64, &destination, pcm24, 4096).is_err(), "a render that is not a .caf file");

    let bad = scratch.path("nan.caf");
    write_render(&bad, &[f32::NAN, 0.2], &[0.1, 0.2], 48_000);
    let mut invalid = DeliveryJob::new(&bad, 2, &destination, settings(ExportEncoding::Float32, 96_000), 4096).unwrap();
    let error = invalid.advance(8).unwrap_err();
    assert_eq!(error.to_string(), "Audio delivery: Non-finite audio cannot be delivered.");
    assert!(invalid.advance(8).is_err(), "a failed job stays failed");
    assert!(!destination.exists(), "a failed delivery became visible");
    let leftovers = std::fs::read_dir(&scratch.0).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(".delivery-")).count();
    assert_eq!(leftovers, 0, "private delivery folders leaked");
    assert_eq!(std::fs::read(&source).unwrap(), original, "failure or cancellation changed the source");
}

#[test]
fn a_render_at_another_rate_delivers_through_the_same_path() {
    use st_samples::export::{ExportFormat, ExportFormatRequest};
    let scratch = Scratch::new("render-rate");
    let source = scratch.path("render.caf");
    let count = 4410;
    let tone: Vec<f32> = (0..count).map(|i| (0.5 * (2.0 * PI * 441.0 * i as f64 / 44_100.0).sin()) as f32).collect();
    write_render(&source, &tone, &tone, 44_100);
    let request: ExportFormatRequest = serde_json::from_str(r#"{"sampleRate":48000,"encoding":"pcm24"}"#).unwrap();
    let format = ExportFormat::resolve(&request, "song", 44_100).unwrap();
    let destination = scratch.path("out.wav");
    let mut job = DeliveryJob::new(&source, count as u64, &destination, DeliverySettings::from_format(format).unwrap(), 4096).unwrap();
    while !job.advance(16).unwrap().complete {}
    let result = job.publish().unwrap();
    assert_eq!(result.output_frames, 4800);
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["inputFrames"], 4410);
    assert_eq!(json["renderSampleRate"], 44_100);
    assert_eq!(json["bitDepth"], 24);
    let delivered = read(&destination);
    let worst = (500..4300).map(|i| (delivered[0][i] as f64 - 0.5 * (2.0 * PI * 441.0 * i as f64 / 48_000.0).sin()).abs()).fold(0f64, f64::max);
    assert!(worst < 2.0 / 8_388_608.0, "44.1 -> 48 kHz delivery error {worst}");
}

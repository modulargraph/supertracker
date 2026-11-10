//! Samples that come with the app rather than with a project, addressed by references a project
//! can carry anywhere:
//!
//! * `builtin:wavetables/<group>/<name>.wav`: the generated wavetables below.
//! * `builtin:drums/<model>/<eight parameters as hex>`: one hit of a drum model, rendered by the
//!   engine (this crate only parses the reference and wraps the rendered audio).
//!
//! Built-in samples are generated in memory (additive synthesis, no recorded or third-party
//! material); nothing is installed on disk. They are never collected, relinked or treated as
//! external media.
//!
//! ```text
//!   Basic   single cycles: Sine, Triangle, Saw, Square, Pulse 25, Pulse 12.5
//!           plus Noise (a two-second loop, played in forward-loop mode)
//!   Morph   32-cycle tables for the Position control
//! ```

use std::f64::consts::PI;
use std::sync::OnceLock;

use serde::Serialize;

use crate::error::{Result, SampleError};
use crate::pcm::DecodedSample;
use crate::wavetable::{detect_frame_size, DEFAULT_FRAME_SIZE};

/// The prefix of every built-in reference.
pub const BUILTIN_PREFIX: &str = "builtin:";
/// Frames per cycle of the built-in tables.
pub const FRAME_SIZE: usize = DEFAULT_FRAME_SIZE;
/// Cycles in a morphing table.
pub const MORPH_FRAMES: usize = 32;
/// Sample rate the built-in samples are declared at.
pub const BUILTIN_SAMPLE_RATE: u32 = 48_000;
/// Names of the engine's drum models, in model order (kick is model 1).
pub const DRUM_MODELS: [&str; 6] = ["kick", "snare", "hihat", "tom", "perc", "cymbal"];

/// True for `builtin:` references.
pub fn is_builtin(path: &str) -> bool {
    path.starts_with(BUILTIN_PREFIX)
}

/// The reference of a built-in wavetable.
pub fn wavetable_reference(group: &str, name: &str) -> String {
    format!("{BUILTIN_PREFIX}wavetables/{group}/{name}.wav")
}

/// The reference of a drum-model hit with eight normalised controls (NaN = the model's default).
pub fn drum_reference(model: &str, parameters: &[f32; 8]) -> String {
    let hex: String = parameters.iter().map(|p| format!("{:08x}", p.to_bits())).collect();
    format!("{BUILTIN_PREFIX}drums/{model}/{hex}")
}

/// The single-cycle table that stands in for a plain oscillator waveform
/// ("sine", "triangle", "saw", "square", "noise"; anything else gives the sine).
pub fn basic_reference(waveform: &str) -> String {
    let name = match waveform {
        "triangle" => "Triangle",
        "saw" => "Saw",
        "square" => "Square",
        "noise" => "Noise",
        _ => "Sine",
    };
    wavetable_reference("Basic", name)
}

/// A parsed `builtin:` reference.
#[derive(Debug, Clone, PartialEq)]
pub enum BuiltinReference {
    /// A generated wavetable.
    Wavetable {
        /// "Basic" or "Morph".
        group: String,
        /// Table name without ".wav".
        name: String,
    },
    /// A rendered drum-model hit.
    Drum {
        /// One of [`DRUM_MODELS`].
        model: String,
        /// 1-based index of the model in [`DRUM_MODELS`].
        model_index: usize,
        /// Eight normalised controls; NaN means the model's default.
        parameters: [f32; 8],
    },
}

/// Parses a `builtin:` reference. Errors name the reference when it is not one this build knows.
pub fn parse_reference(path: &str) -> Result<BuiltinReference> {
    let unknown = || SampleError::NotFound(format!("Unknown built-in sample: {path}."));
    let rest = path.strip_prefix(BUILTIN_PREFIX).ok_or_else(unknown)?;
    let parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        ["wavetables", group, file] => {
            let name = file.strip_suffix(".wav").unwrap_or(file);
            if catalogue().iter().any(|table| table.group == *group && table.name == name) && file.ends_with(".wav") {
                Ok(BuiltinReference::Wavetable { group: group.to_string(), name: name.to_string() })
            } else {
                Err(SampleError::NotFound(format!("Unknown built-in wavetable: {group}/{file}.")))
            }
        }
        ["drums", model, hex] if hex.len() == 64 && hex.is_ascii() => {
            let model_index = DRUM_MODELS.iter().position(|known| known == model).ok_or_else(unknown)? + 1;
            let mut parameters = [0f32; 8];
            for (index, parameter) in parameters.iter_mut().enumerate() {
                *parameter = f32::from_bits(u32::from_str_radix(&hex[index * 8..index * 8 + 8], 16).unwrap_or(0));
            }
            Ok(BuiltinReference::Drum { model: model.to_string(), model_index, parameters })
        }
        _ => Err(unknown()),
    }
}

/// Renders a drum-model hit as mono 48 kHz PCM: `(model index 1...6, eight controls)`.
/// Supplied by the caller that owns the engine.
pub type DrumRenderer<'a> = &'a dyn Fn(usize, &[f32; 8]) -> Vec<f32>;

/// Loads the sample a `builtin:` reference names. Drum references need `drum_renderer`.
pub fn load_builtin(reference: &str, drum_renderer: Option<DrumRenderer<'_>>) -> Result<DecodedSample> {
    let mono = match parse_reference(reference)? {
        BuiltinReference::Wavetable { group, name } => {
            let table = catalogue().iter().find(|table| table.group == group && table.name == name).expect("parsed against the catalogue");
            let mut sample = from_mono(reference, table.samples().to_vec());
            sample.cycle_frames = if table.is_wavetable() { Some(FRAME_SIZE) } else { None };
            return Ok(sample);
        }
        BuiltinReference::Drum { model, model_index, parameters } => {
            let render = drum_renderer.ok_or_else(|| SampleError::invalid("Built-in drum samples need the audio engine."))?;
            let samples = render(model_index, &parameters);
            if samples.is_empty() {
                return Err(SampleError::invalid(format!("The built-in {model} drum could not be rendered.")));
            }
            samples
        }
    };
    let mut sample = from_mono(reference, mono.into_iter().map(|v| if v.is_finite() { v } else { 0.0 }).collect());
    sample.cycle_frames = detect_frame_size(None, sample.left.len());
    Ok(sample)
}

fn from_mono(reference: &str, samples: Vec<f32>) -> DecodedSample {
    DecodedSample::new(reference, BUILTIN_SAMPLE_RATE as f64, samples.clone(), samples, 1)
}

/// One row of the sampler's wavetable picker.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WavetableListing {
    /// "Basic" or "Morph".
    pub group: String,
    /// Table name.
    pub name: String,
    /// The `builtin:` reference to store in a project.
    pub path: String,
    /// Cycles in the table; 0 for Noise, which is a loop.
    pub frames: usize,
    /// Frames per cycle; 0 for Noise.
    pub frame_size: usize,
    /// False for Noise.
    pub wavetable: bool,
}

/// The built-in tables, in picker order.
pub fn wavetable_list() -> Vec<WavetableListing> {
    catalogue()
        .iter()
        .map(|table| WavetableListing {
            group: table.group.to_string(),
            name: table.name.to_string(),
            path: wavetable_reference(table.group, table.name),
            frames: if table.is_wavetable() { table.cycles } else { 0 },
            frame_size: if table.is_wavetable() { FRAME_SIZE } else { 0 },
            wavetable: table.is_wavetable(),
        })
        .collect()
}

struct Table {
    group: &'static str,
    name: &'static str,
    cycles: usize,
    generate: fn() -> Vec<f32>,
    samples: OnceLock<Vec<f32>>,
}

impl Table {
    /// Noise is a loop, not a cycle table.
    fn is_wavetable(&self) -> bool {
        !(self.group == "Basic" && self.name == "Noise")
    }

    /// Generated on first use and kept.
    fn samples(&self) -> &[f32] {
        self.samples.get_or_init(self.generate)
    }
}

fn catalogue() -> &'static [Table] {
    static TABLES: OnceLock<Vec<Table>> = OnceLock::new();
    TABLES.get_or_init(|| {
        let table = |group, name, cycles, generate: fn() -> Vec<f32>| Table { group, name, cycles, generate, samples: OnceLock::new() };
        vec![
            table("Basic", "Sine", 1, || additive(256, |n| if n == 1 { 1.0 } else { 0.0 })),
            table("Basic", "Triangle", 1, || {
                additive(256, |n| if n % 2 == 1 { (if n % 4 == 1 { 1.0 } else { -1.0 }) / (n * n) as f64 } else { 0.0 })
            }),
            table("Basic", "Saw", 1, || additive(256, |n| 1.0 / n as f64)),
            table("Basic", "Square", 1, || additive(256, |n| if n % 2 == 1 { 1.0 / n as f64 } else { 0.0 })),
            table("Basic", "Pulse 25", 1, || pulse(0.25)),
            table("Basic", "Pulse 12.5", 1, || pulse(0.125)),
            table("Basic", "Noise", 1, || noise(96_000)),
            table("Morph", "Sine to Saw", MORPH_FRAMES, || morph(|t| additive(256, |n| if n == 1 { 1.0 } else { t / n as f64 }))),
            table("Morph", "Pulse Width", MORPH_FRAMES, || morph(|t| pulse(0.5 - 0.47 * t))),
            table("Morph", "Harmonic Sweep", MORPH_FRAMES, || {
                morph(|t| {
                    let position = 1.0 + t * 31.0;
                    let lower = position as usize;
                    let blend = position - lower as f64;
                    additive(256, |n| if n == lower { 1.0 - blend } else if n == lower + 1 { blend } else { 0.0 })
                })
            }),
            table("Morph", "Vowels", MORPH_FRAMES, || morph(vowel)),
            table("Morph", "Wavefold", MORPH_FRAMES, || morph(|t| fold(1.0 + 7.0 * t))),
            table("Morph", "Sync Sweep", MORPH_FRAMES, || morph(|t| sync(1.0 + 7.0 * t))),
        ]
    })
}

/// One sine cycle; harmonic n at sample i is `sine_table()[(n * i) % FRAME_SIZE]`.
fn sine_table() -> &'static [f64] {
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    TABLE.get_or_init(|| (0..FRAME_SIZE).map(|i| (2.0 * PI * i as f64 / FRAME_SIZE as f64).sin()).collect())
}

/// Band-limited cycle from harmonic amplitudes (sine phase), Lanczos-smoothed against ringing
/// and normalised to a 0.9 peak.
fn additive(harmonics: usize, amplitude: impl Fn(usize) -> f64) -> Vec<f32> {
    let sine = sine_table();
    let mut values = vec![0f64; FRAME_SIZE];
    for n in 1..=harmonics {
        let a = amplitude(n);
        if a == 0.0 {
            continue;
        }
        let x = PI * n as f64 / (harmonics + 1) as f64;
        let weight = a * if n == 1 { 1.0 } else { x.sin() / x };
        for (i, value) in values.iter_mut().enumerate() {
            *value += weight * sine[(n * i) % FRAME_SIZE];
        }
    }
    normalized(&values, 0.9)
}

/// Pulse = saw(phase) - saw(phase + width), the width rounded to one table sample.
fn pulse(width: f64) -> Vec<f32> {
    let sine = sine_table();
    let shift = (width * FRAME_SIZE as f64).round() as usize;
    let mut values = vec![0f64; FRAME_SIZE];
    for n in 1..=256usize {
        let x = PI * n as f64 / 257.0;
        let a = (if n == 1 { 1.0 } else { x.sin() / x }) / n as f64;
        for (i, value) in values.iter_mut().enumerate() {
            *value += a * (sine[(n * i) % FRAME_SIZE] - sine[(n * (i + shift)) % FRAME_SIZE]);
        }
    }
    let mean = values.iter().sum::<f64>() / FRAME_SIZE as f64;
    let centred: Vec<f64> = values.iter().map(|v| v - mean).collect();
    normalized(&centred, 0.9)
}

/// Spoken-vowel formants (A, E, I, O, U) as Gaussian bumps over harmonics of 110 Hz.
fn vowel(t: f64) -> Vec<f32> {
    const FORMANTS: [(f64, f64); 5] = [(800.0, 1150.0), (400.0, 1600.0), (300.0, 2300.0), (500.0, 900.0), (350.0, 700.0)];
    let position = t * (FORMANTS.len() - 1) as f64;
    let lower = (position as usize).min(FORMANTS.len() - 2);
    let blend = position - lower as f64;
    let f1 = FORMANTS[lower].0 + (FORMANTS[lower + 1].0 - FORMANTS[lower].0) * blend;
    let f2 = FORMANTS[lower].1 + (FORMANTS[lower + 1].1 - FORMANTS[lower].1) * blend;
    additive(64, |n| {
        let frequency = 110.0 * n as f64;
        let bump = |center: f64, width: f64| (-((frequency - center) / width).powi(2)).exp();
        (bump(f1, 120.0) + 0.6 * bump(f2, 160.0) + 0.05) / (n as f64).sqrt()
    })
}

fn fold(gain: f64) -> Vec<f32> {
    let values: Vec<f64> = (0..FRAME_SIZE).map(|i| (gain * (2.0 * PI * i as f64 / FRAME_SIZE as f64).sin() * PI / 2.0).sin()).collect();
    normalized(&values, 0.9)
}

/// Hard-synced saw: a slave saw at `ratio` times the cycle, reset every cycle.
fn sync(ratio: f64) -> Vec<f32> {
    let raw: Vec<f64> = (0..FRAME_SIZE)
        .map(|i| {
            let phase = i as f64 / FRAME_SIZE as f64 * ratio;
            2.0 * (phase - phase.floor()) - 1.0
        })
        .collect();
    // A short moving average softens the reset step a little.
    let smoothed: Vec<f64> = (0..FRAME_SIZE).map(|i| (raw[(i + FRAME_SIZE - 1) % FRAME_SIZE] + 2.0 * raw[i] + raw[(i + 1) % FRAME_SIZE]) / 4.0).collect();
    let mean = smoothed.iter().sum::<f64>() / FRAME_SIZE as f64;
    let centred: Vec<f64> = smoothed.iter().map(|v| v - mean).collect();
    normalized(&centred, 0.9)
}

fn noise(frames: usize) -> Vec<f32> {
    let mut state: u32 = 0x1234_5678;
    let values: Vec<f64> = (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f64 / u32::MAX as f64 * 2.0 - 1.0
        })
        .collect();
    normalized(&values, 0.7)
}

fn morph(frame: impl Fn(f64) -> Vec<f32>) -> Vec<f32> {
    (0..MORPH_FRAMES).flat_map(|index| frame(index as f64 / (MORPH_FRAMES - 1) as f64)).collect()
}

fn normalized(values: &[f64], peak: f64) -> Vec<f32> {
    let maximum = values.iter().fold(0f64, |m, v| m.max(v.abs()));
    let gain = if maximum > 0.0 { peak / maximum } else { 0.0 };
    values.iter().map(|v| (v * gain) as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harmonic(cycle: &[f32], n: usize) -> f64 {
        let (mut s, mut c) = (0.0, 0.0);
        for (i, &v) in cycle.iter().enumerate() {
            let phase = 2.0 * PI * (n * i) as f64 / cycle.len() as f64;
            s += v as f64 * phase.sin();
            c += v as f64 * phase.cos();
        }
        2.0 * (s * s + c * c).sqrt() / cycle.len() as f64
    }

    #[test]
    fn listing_and_references() {
        let list = wavetable_list();
        assert_eq!(list.len(), 13);
        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(&names[..7], ["Sine", "Triangle", "Saw", "Square", "Pulse 25", "Pulse 12.5", "Noise"]);
        let noise = &list[6];
        assert!(!noise.wavetable && noise.frames == 0 && noise.frame_size == 0);
        let morph = list.iter().find(|t| t.name == "Vowels").unwrap();
        assert_eq!((morph.frames, morph.frame_size, morph.wavetable), (32, 2048, true));
        assert_eq!(morph.path, "builtin:wavetables/Morph/Vowels.wav");
        let json = serde_json::to_value(morph).unwrap();
        assert_eq!(json["frameSize"], 2048);
        assert_eq!(basic_reference("saw"), "builtin:wavetables/Basic/Saw.wav");
        assert_eq!(basic_reference("anything"), "builtin:wavetables/Basic/Sine.wav");
        assert!(is_builtin("builtin:wavetables/Basic/Saw.wav") && !is_builtin("/a/builtin:x.wav"));
    }

    #[test]
    fn basic_cycles_have_the_expected_spectra() {
        let load = |name: &str| load_builtin(&wavetable_reference("Basic", name), None).unwrap();
        let sine = load("Sine");
        assert_eq!((sine.left.len(), sine.cycle_frames, sine.original_channels, sine.sample_rate), (2048, Some(2048), 1, 48_000.0));
        assert_eq!(sine.left, sine.right);
        assert!((harmonic(&sine.left, 1) - 0.9).abs() < 1e-4 && harmonic(&sine.left, 2) < 1e-6);
        let saw = load("Saw");
        let peak = saw.left.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.9).abs() < 1e-6, "normalised to 0.9: {peak}");
        // Harmonics fall as 1/n (before the gentle Lanczos roll-off).
        let ratio = harmonic(&saw.left, 2) / harmonic(&saw.left, 1);
        assert!((ratio - 0.5).abs() < 0.01, "saw 2nd/1st harmonic {ratio}");
        let square = load("Square");
        assert!(harmonic(&square.left, 2) < 1e-6 && (harmonic(&square.left, 3) / harmonic(&square.left, 1) - 1.0 / 3.0).abs() < 0.01);
        let triangle = load("Triangle");
        assert!((harmonic(&triangle.left, 3) / harmonic(&triangle.left, 1) - 1.0 / 9.0).abs() < 0.01);
        let pulse = load("Pulse 25");
        // A 25 % pulse has no 4th harmonic and no DC.
        assert!(harmonic(&pulse.left, 4) < 1e-6 && (pulse.left.iter().map(|&v| v as f64).sum::<f64>() / 2048.0).abs() < 1e-6);
        let noise = load("Noise");
        assert_eq!((noise.left.len(), noise.cycle_frames), (96_000, None));
        assert!((noise.left.iter().fold(0f32, |m, v| m.max(v.abs())) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn morph_tables_hold_32_cycles_that_change() {
        for name in ["Sine to Saw", "Pulse Width", "Harmonic Sweep", "Vowels", "Wavefold", "Sync Sweep"] {
            let table = load_builtin(&wavetable_reference("Morph", name), None).unwrap();
            assert_eq!((table.left.len(), table.cycle_frames), (32 * 2048, Some(2048)), "{name}");
            assert!(table.left.iter().all(|v| v.is_finite() && v.abs() <= 0.9001), "{name} bounded");
            assert!(table.left[..2048] != table.left[31 * 2048..], "{name} morphs");
            for cycle in table.left.chunks(2048) {
                let peak = cycle.iter().fold(0f32, |m, v| m.max(v.abs()));
                assert!((peak - 0.9).abs() < 1e-5, "{name} cycle peak {peak}");
            }
        }
        let sweep = load_builtin("builtin:wavetables/Morph/Harmonic Sweep.wav", None).unwrap();
        assert!(harmonic(&sweep.left[..2048], 1) > 0.89 && harmonic(&sweep.left[31 * 2048..], 32) > 0.89);
        let sine_to_saw = load_builtin("builtin:wavetables/Morph/Sine to Saw.wav", None).unwrap();
        assert!(harmonic(&sine_to_saw.left[..2048], 2) < 1e-6 && harmonic(&sine_to_saw.left[31 * 2048..], 2) > 0.2);
    }

    #[test]
    fn drum_references_round_trip_through_the_renderer() {
        let parameters = [0.5, f32::NAN, 0.0, 1.0, 0.25, 0.75, 0.125, 0.875];
        let reference = drum_reference("snare", &parameters);
        assert!(reference.starts_with("builtin:drums/snare/") && reference.len() == "builtin:drums/snare/".len() + 64);
        match parse_reference(&reference).unwrap() {
            BuiltinReference::Drum { model, model_index, parameters: parsed } => {
                assert_eq!((model.as_str(), model_index), ("snare", 2));
                assert!(parsed[1].is_nan() && parsed[0] == 0.5 && parsed[7] == 0.875);
            }
            other => panic!("unexpected {other:?}"),
        }
        let render = |model: usize, values: &[f32; 8]| vec![model as f32 * 0.1, values[0], f32::INFINITY];
        let sample = load_builtin(&reference, Some(&render)).unwrap();
        assert_eq!(sample.left, vec![0.2, 0.5, 0.0]);
        assert_eq!((sample.path.as_str(), sample.original_channels), (reference.as_str(), 1));
        assert!(load_builtin(&reference, None).unwrap_err().to_string().contains("audio engine"));
        let silent = |_: usize, _: &[f32; 8]| Vec::new();
        assert!(load_builtin(&reference, Some(&silent)).is_err());
    }

    #[test]
    fn unknown_references_are_named() {
        for bad in ["builtin:wavetables/Basic/Nope.wav", "builtin:wavetables/Basic/Saw", "builtin:drums/gong/00", "builtin:other", "/plain/path.wav", "builtin:drums/kick/zz"] {
            let error = load_builtin(bad, None).unwrap_err();
            assert!(matches!(error, SampleError::NotFound(_)), "{bad}: {error}");
            assert!(error.to_string().contains("built-in"), "{bad}: {error}");
        }
    }
}

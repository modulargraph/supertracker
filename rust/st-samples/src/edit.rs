//! The sample editor's region edits: trim, reverse, normalize, gain, fades, DC removal, silence
//! and time stretch.
//!
//! Edits never modify the file they started from. [`apply_edit`] works in memory;
//! [`edit_to_file`] also writes the result as a new project-owned 24-bit WAV file.

use std::ops::Range;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, SampleError};
use crate::op::Operation;
use crate::paths::unique_suffix;
use crate::pcm::DecodedSample;
use crate::stretch::{stretch, stretch_ratio};
use crate::wav::{WavEncoding, WavSpec, WavWriter};

/// What `sample.edit` asks for. Positions are fractions (0...1) of the sample.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EditRequest {
    /// "trim" (or "crop"), "reverse", "normalize", "gain", "fadeIn", "fadeOut", "removeDC",
    /// "silence" or "stretch". Default "trim".
    pub operation: Option<String>,
    /// Older name of `operation`, used when that is absent.
    pub action: Option<String>,
    /// Region start (default 0).
    pub start: Option<f64>,
    /// Region end (default 1).
    pub end: Option<f64>,
    /// Normalize: the peak to reach (default 0.98).
    pub peak: Option<f64>,
    /// Gain: the change in dB, limited to -96...+24 (default 0).
    pub decibels: Option<f64>,
    /// Fades: "linear" (default), "smooth" (S-curve) or "exponential" (perceptually even).
    pub curve: Option<String>,
    /// Stretch: output duration / input duration, 0.25...4.
    pub ratio: Option<f64>,
    /// Stretch: tempo of the material, used with `targetBPM` when `ratio` is absent.
    #[serde(rename = "sourceBPM")]
    pub source_bpm: Option<f64>,
    /// Stretch: tempo to reach.
    #[serde(rename = "targetBPM")]
    pub target_bpm: Option<f64>,
    /// Folder for the new file; see [`crate::paths::edit_directory`] for the default.
    #[serde(rename = "outputDirectory")]
    pub output_directory: Option<String>,
}

/// The shape of a fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeCurve {
    /// Gain proportional to position.
    Linear,
    /// Raised-cosine S-curve: no kink at either end.
    Smooth,
    /// Cubic: slow near silence, which sounds even to the ear.
    Exponential,
}

impl FadeCurve {
    /// "smooth" and "exponential" by name; anything else is linear.
    pub fn from_name(name: Option<&str>) -> FadeCurve {
        match name {
            Some("smooth") => FadeCurve::Smooth,
            Some("exponential") => FadeCurve::Exponential,
            _ => FadeCurve::Linear,
        }
    }

    /// Gain at position `x` (0 = silent end, 1 = full level).
    pub fn gain(self, x: f32) -> f32 {
        match self {
            FadeCurve::Linear => x,
            FadeCurve::Smooth => 0.5 - 0.5 * (std::f32::consts::PI * x).cos(),
            FadeCurve::Exponential => x * x * x,
        }
    }
}

/// A validated edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditOperation {
    /// Keep only the region.
    Trim,
    /// Play the region backwards.
    Reverse,
    /// Scale the region so its peak becomes `peak`.
    Normalize {
        /// Target peak magnitude.
        peak: f32,
    },
    /// Scale the region by a factor.
    Gain {
        /// Linear gain.
        factor: f32,
    },
    /// Fade the region in from silence.
    FadeIn(FadeCurve),
    /// Fade the region out to silence.
    FadeOut(FadeCurve),
    /// Subtract each channel's mean over the region.
    RemoveDc,
    /// Replace the region with silence.
    Silence,
    /// Time-stretch the region (the rest keeps its length).
    Stretch {
        /// Output duration / input duration of the region.
        ratio: f64,
    },
}

impl EditRequest {
    /// The operation name as requested ("trim" when none was given).
    pub fn operation_name(&self) -> &str {
        self.operation.as_deref().or(self.action.as_deref()).unwrap_or("trim")
    }

    /// Validates the operation and its parameters.
    pub fn resolve(&self) -> Result<EditOperation> {
        Ok(match self.operation_name() {
            "trim" | "crop" => EditOperation::Trim,
            "reverse" => EditOperation::Reverse,
            "normalize" => EditOperation::Normalize { peak: self.peak.filter(|p| p.is_finite()).unwrap_or(0.98) as f32 },
            "gain" => {
                let decibels = self.decibels.filter(|d| !d.is_nan()).unwrap_or(0.0).clamp(-96.0, 24.0);
                EditOperation::Gain { factor: 10f64.powf(decibels / 20.0) as f32 }
            }
            "fadeIn" => EditOperation::FadeIn(FadeCurve::from_name(self.curve.as_deref())),
            "fadeOut" => EditOperation::FadeOut(FadeCurve::from_name(self.curve.as_deref())),
            "removeDC" => EditOperation::RemoveDc,
            "silence" => EditOperation::Silence,
            "stretch" => EditOperation::Stretch { ratio: stretch_ratio(self.ratio, self.source_bpm, self.target_bpm)? },
            other => return Err(SampleError::invalid(format!("Unknown sample edit: {other}."))),
        })
    }
}

/// The frames an edit covers. Checks the sample first (1...100 million frames, equal channel
/// lengths, positive finite rate, finite audio), then clamps finite positions to the sample;
/// the region always holds at least one frame.
pub fn selection_range(source: &DecodedSample, start: Option<f64>, end: Option<f64>) -> Result<Range<usize>> {
    source.validate().map_err(|_| SampleError::invalid("The sample has invalid or empty audio data. Nothing was edited."))?;
    let count = source.left.len();
    let position = |key: &str, value: Option<f64>, fallback: f64| match value {
        None => Ok(fallback),
        Some(number) if number.is_finite() => Ok(number.clamp(0.0, 1.0)),
        Some(_) => Err(SampleError::invalid(format!("Sample {key} must be a finite numeric position. Nothing was edited."))),
    };
    let (lower, upper) = (position("start", start, 0.0)?, position("end", end, 1.0)?);
    let first = ((lower * count as f64) as usize).min(count - 1);
    let last = ((upper * count as f64) as usize).min(count).max(first + 1);
    Ok(first..last)
}

/// Applies an edit in memory and returns the edited audio under the source's reference.
/// The source is not modified.
pub fn apply_edit(source: &DecodedSample, request: &EditRequest, operation: &mut Operation<'_>) -> Result<DecodedSample> {
    let edit = request.resolve()?;
    let range = selection_range(source, request.start, request.end)?;
    let (start, end) = (range.start, range.end);
    if let EditOperation::Stretch { ratio } = edit {
        let selection = source.with_audio(source.left[range.clone()].to_vec(), source.right[range.clone()].to_vec());
        let stretched = stretch(&selection, ratio, operation)?;
        let splice = |whole: &[f32], middle: &[f32]| [&whole[..start], middle, &whole[end..]].concat();
        return Ok(source.with_audio(splice(&source.left, &stretched.left), splice(&source.right, &stretched.right)));
    }
    if edit == EditOperation::Trim {
        return Ok(source.with_audio(source.left[range.clone()].to_vec(), source.right[range].to_vec()));
    }
    let (mut left, mut right) = (source.left.clone(), source.right.clone());
    let scale = |left: &mut [f32], right: &mut [f32], gain: f32| {
        left.iter_mut().chain(right.iter_mut()).for_each(|v| *v *= gain);
    };
    match edit {
        EditOperation::Reverse => {
            left[range.clone()].reverse();
            right[range].reverse();
        }
        EditOperation::Normalize { peak } => {
            let level = left[range.clone()].iter().chain(&right[range.clone()]).fold(0f32, |m, v| m.max(v.abs()));
            scale(&mut left[range.clone()], &mut right[range], peak / level.max(0.000_001));
        }
        EditOperation::Gain { factor } => scale(&mut left[range.clone()], &mut right[range], factor),
        EditOperation::FadeIn(curve) | EditOperation::FadeOut(curve) => {
            let fade_in = matches!(edit, EditOperation::FadeIn(_));
            let length = (end - start - 1).max(1) as f32;
            for i in range {
                let t = (i - start) as f32 / length;
                let gain = curve.gain(if fade_in { t } else { 1.0 - t });
                left[i] *= gain;
                right[i] *= gain;
            }
        }
        EditOperation::RemoveDc => {
            let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len() as f32;
            let (mean_left, mean_right) = (mean(&left[range.clone()]), mean(&right[range.clone()]));
            left[range.clone()].iter_mut().for_each(|v| *v -= mean_left);
            right[range].iter_mut().for_each(|v| *v -= mean_right);
        }
        EditOperation::Silence => {
            left[range.clone()].fill(0.0);
            right[range].fill(0.0);
        }
        EditOperation::Trim | EditOperation::Stretch { .. } => unreachable!("handled above"),
    }
    Ok(source.with_audio(left, right))
}

/// Writes `sample` as a stereo 24-bit WAV file at `path`. The file appears under its final
/// name only when it is complete.
pub fn write_sample(sample: &DecodedSample, path: &Path) -> Result<()> {
    let rate = sample.sample_rate.round();
    if !(rate >= 1.0 && rate <= u32::MAX as f64) || sample.left.len() != sample.right.len() {
        return Err(SampleError::invalid("The sample has invalid audio data and cannot be written."));
    }
    let leaf = path.file_name().and_then(|n| n.to_str()).unwrap_or("sample.wav");
    let partial = path.with_file_name(format!(".{leaf}.{}.partial", unique_suffix()));
    let written = (|| {
        let mut writer = WavWriter::create_new(&partial, WavSpec::stereo(rate as u32, WavEncoding::Pcm24), &[])?;
        for (left, right) in sample.left.chunks(65_536).zip(sample.right.chunks(65_536)) {
            writer.write_planar(&[left, right])?;
        }
        let (out, _) = writer.finalize()?;
        out.into_inner().map_err(|e| SampleError::io("Write sample file", e.error()))?;
        std::fs::rename(&partial, path).map_err(|e| SampleError::io("Write sample file", &e))
    })();
    if written.is_err() {
        std::fs::remove_file(&partial).ok();
    }
    written
}

/// Applies an edit and writes the result to a new file `<name>-<operation>-<suffix>.wav` in
/// `directory` (created if needed). Returns the edited sample with the new file as its path.
///
/// The source file is never touched, and a failed or cancelled edit leaves no file behind.
pub fn edit_to_file(source: &DecodedSample, request: &EditRequest, directory: &Path, operation: &mut Operation<'_>) -> Result<DecodedSample> {
    let mut edited = apply_edit(source, request, operation)?;
    operation.check()?;
    std::fs::create_dir_all(directory).map_err(|e| SampleError::io("Create the sample edit folder", &e))?;
    let destination = directory.join(format!("{}-{}-{}.wav", source.name(), request.operation_name(), unique_suffix()));
    write_sample(&edited, &destination)?;
    edited.path = destination.to_string_lossy().into_owned();
    edited.original_channels = 2;
    Ok(edited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_file;

    const VALUES: [f32; 8] = [-1.0, -0.5, 0.1, 0.8, 0.7, 0.2, -0.1, -0.8];

    fn small() -> DecodedSample {
        DecodedSample::new("synthetic", 1000.0, VALUES.to_vec(), VALUES.to_vec(), 2)
    }

    #[test]
    fn selection_bounds() {
        let sample = small();
        assert_eq!(selection_range(&sample, None, None).unwrap(), 0..8, "a missing selection is the whole sample");
        assert_eq!(selection_range(&sample, Some(0.25), Some(0.75)).unwrap(), 2..6);
        assert_eq!(selection_range(&sample, Some(-1e100), Some(1e100)).unwrap(), 0..8, "huge finite positions clamp");
        assert_eq!(selection_range(&sample, Some(1e100), Some(-1e100)).unwrap(), 7..8, "a reversed selection keeps one frame");
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(selection_range(&sample, Some(invalid), None).is_err());
            assert!(selection_range(&sample, None, Some(invalid)).is_err());
        }
        let empty = DecodedSample::new("empty", 48_000.0, vec![], vec![], 2);
        let unbalanced = DecodedSample::new("bad", 1000.0, VALUES.to_vec(), vec![], 2);
        let no_rate = DecodedSample::new("bad", 0.0, VALUES.to_vec(), VALUES.to_vec(), 2);
        let nan_rate = DecodedSample::new("bad", f64::NAN, VALUES.to_vec(), VALUES.to_vec(), 2);
        let nan_audio = DecodedSample::new("bad", 1000.0, vec![f32::NAN], vec![0.0], 2);
        for source in [empty, unbalanced, no_rate, nan_rate, nan_audio] {
            let error = selection_range(&source, None, None).unwrap_err();
            assert!(error.to_string().contains("Nothing was edited"));
        }
    }

    #[test]
    fn requests_from_json() {
        let request: EditRequest =
            serde_json::from_str(r#"{"instrument":0,"path":"/x.wav","operation":"stretch","sourceBPM":120,"targetBPM":90,"outputDirectory":"/tmp/x","start":0.1}"#).unwrap();
        assert_eq!(request.resolve().unwrap(), EditOperation::Stretch { ratio: 120.0 / 90.0 });
        assert_eq!(request.output_directory.as_deref(), Some("/tmp/x"));
        let legacy: EditRequest = serde_json::from_str(r#"{"action":"reverse"}"#).unwrap();
        assert_eq!(legacy.resolve().unwrap(), EditOperation::Reverse);
        assert_eq!(EditRequest::default().resolve().unwrap(), EditOperation::Trim);
        let unknown: EditRequest = serde_json::from_str(r#"{"operation":"explode"}"#).unwrap();
        assert_eq!(unknown.resolve().unwrap_err().to_string(), "Unknown sample edit: explode.");
        assert!(serde_json::from_str::<EditRequest>(r#"{"start":"not numeric"}"#).is_err());
        assert!(serde_json::from_str::<EditRequest>(r#"{"end":true}"#).is_err());
        let gain: EditRequest = serde_json::from_str(r#"{"operation":"gain","decibels":100}"#).unwrap();
        assert_eq!(gain.resolve().unwrap(), EditOperation::Gain { factor: 10f64.powf(24.0 / 20.0) as f32 });
    }

    #[test]
    fn in_memory_operations() {
        let sample = small();
        let run = |json: &str| apply_edit(&sample, &serde_json::from_str(json).unwrap(), &mut Operation::new()).unwrap();
        let dc = run(r#"{"operation":"removeDC","start":0.5}"#);
        assert!(dc.left[4..].iter().sum::<f32>().abs() < 1e-6 && dc.left[..4] == VALUES[..4]);
        let silence = run(r#"{"operation":"silence","start":0.25,"end":0.5}"#);
        assert_eq!(silence.left, vec![-1.0, -0.5, 0.0, 0.0, 0.7, 0.2, -0.1, -0.8]);
        let crop = run(r#"{"operation":"crop","start":0.5}"#);
        assert_eq!(crop.left, VALUES[4..]);
        let stretched = run(r#"{"operation":"stretch","ratio":2,"start":0.25,"end":0.75}"#);
        assert_eq!(stretched.left.len(), 2 + 8 + 2, "only the region changes length");
        assert!(stretched.left[..2] == VALUES[..2] && stretched.left[10..] == VALUES[6..]);
        assert_eq!(sample.left, VALUES, "the source is untouched");
        // A silent region cannot be normalised into something else.
        let quiet = DecodedSample::new("quiet", 1000.0, vec![0.0; 4], vec![0.0; 4], 2);
        let normalized = apply_edit(&quiet, &serde_json::from_str(r#"{"operation":"normalize"}"#).unwrap(), &mut Operation::new()).unwrap();
        assert!(normalized.left.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn region_edits_write_new_files_and_leave_the_source_alone() {
        let folder = std::env::temp_dir().join(format!("st-samples-edit-{}-{}", std::process::id(), unique_suffix()));
        std::fs::create_dir_all(&folder).unwrap();
        let source_path = folder.join("source.wav");
        let ramp: Vec<f32> = (0..4800).map(|i| i as f32 / 4800.0 * 0.5).collect();
        write_sample(&DecodedSample::new("generated.wav", 48_000.0, ramp.clone(), ramp.iter().map(|v| -v).collect(), 2), &source_path).unwrap();
        let original = std::fs::read(&source_path).unwrap();
        let decoded = decode_file(source_path.to_str().unwrap(), &source_path, &mut Operation::new()).unwrap();
        assert_eq!(decoded.left.len(), 4800);
        let output = folder.join("out");
        let edit = |json: &str| {
            let request: EditRequest = serde_json::from_str(json).unwrap();
            let result = edit_to_file(&decoded, &request, &output, &mut Operation::new()).unwrap();
            assert!(result.path != decoded.path && Path::new(&result.path).is_file(), "an edit writes a new file");
            let leaf = Path::new(&result.path).file_name().unwrap().to_str().unwrap().to_string();
            assert!(leaf.starts_with(&format!("source-{}-", request.operation_name())) && leaf.ends_with(".wav"), "{leaf}");
            let reread = decode_file(&result.path, Path::new(&result.path), &mut Operation::new()).unwrap();
            assert_eq!(reread.left.len(), result.left.len(), "the written edit decodes with the same length");
            assert!(reread.left.iter().zip(&result.left).all(|(a, b)| (a - b).abs() <= 1.0 / 8_388_608.0), "24-bit file matches the edit");
            reread
        };
        let trimmed = edit(r#"{"operation":"trim","start":0.25,"end":0.75}"#);
        assert!(trimmed.left.len() == 2400 && (trimmed.left[0] - 0.125).abs() < 1e-4, "trim keeps exactly the region");
        let normalized = edit(r#"{"operation":"normalize","start":0.0,"end":0.5,"peak":1.0}"#);
        assert!((normalized.left[2399] - 1.0).abs() < 1e-3 && (normalized.left[4799] - ramp[4799]).abs() < 1e-4, "normalize scales only the region");
        let reversed = edit(r#"{"operation":"reverse","start":0.5,"end":1.0}"#);
        assert!((reversed.left[2400] - ramp[4799]).abs() < 1e-4 && (reversed.left[100] - ramp[100]).abs() < 1e-4, "reverse flips only the region");
        let quieter = edit(r#"{"operation":"gain","decibels":-6.0206}"#);
        assert!((quieter.left[4000] - ramp[4000] / 2.0).abs() < 1e-3 && (quieter.right[4000] + ramp[4000] / 2.0).abs() < 1e-3, "gain applies to both channels");
        for curve in ["linear", "smooth", "exponential"] {
            let faded = edit(&format!(r#"{{"operation":"fadeIn","start":0.0,"end":0.5,"curve":"{curve}"}}"#));
            assert!(faded.left[0].abs() < 1e-6 && (faded.left[2399] - ramp[2399]).abs() < 1e-3 && (faded.left[3000] - ramp[3000]).abs() < 1e-4, "{curve} fade-in");
            let out = edit(&format!(r#"{{"operation":"fadeOut","start":0.5,"end":1.0,"curve":"{curve}"}}"#));
            assert!(out.left[4799].abs() < 1e-6 && (out.left[2400] - ramp[2400]).abs() < 1e-3, "{curve} fade-out ends silent");
        }
        let half = edit(r#"{"operation":"fadeIn","start":0.0,"end":1.0,"curve":"exponential"}"#);
        assert!(half.left[2400] < ramp[2400] * 0.2, "an exponential fade is slower at the start than a linear one");
        assert_eq!(std::fs::read(&source_path).unwrap(), original, "source file bytes are unchanged after all edits");

        // A failed edit leaves nothing behind.
        let before = std::fs::read_dir(&output).unwrap().count();
        let bad: EditRequest = serde_json::from_str(r#"{"operation":"stretch","ratio":9}"#).unwrap();
        assert!(edit_to_file(&decoded, &bad, &output, &mut Operation::new()).is_err());
        let token = crate::CancelToken::new();
        token.cancel();
        let slow: EditRequest = serde_json::from_str(r#"{"operation":"stretch","ratio":2}"#).unwrap();
        assert!(edit_to_file(&decoded, &slow, &output, &mut Operation::new().with_cancel(&token)).unwrap_err().is_cancelled());
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), before);
        assert!(std::fs::read_dir(&output).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains("partial")));
        std::fs::remove_dir_all(&folder).ok();
    }
}

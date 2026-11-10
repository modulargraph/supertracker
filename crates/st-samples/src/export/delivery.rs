//! Delivery: turning a finished float render into the WAV file that was asked for.
//!
//! The engine, transport and plug-in graph run at the render rate. A [`DeliveryJob`] reads the
//! intermediate render ([`super::caf`]), converts the sample rate when the delivery rate
//! differs (mastering-quality converter, no added delay, exact length), encodes, verifies the
//! written header and only then lets the file be published. It works in bounded steps so the
//! caller can report progress and cancel between them.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::caf::RenderFileReader;
use super::format::{ExportEncoding, ExportFormat, ExportMetadata, RENDER_SAMPLE_RATE, SUPPORTED_SAMPLE_RATES};
use super::fsx;
use crate::error::{Result, SampleError};
use crate::paths::unique_token;
use crate::resample::{resampled_frames, ResampleQuality, Resampler};
use crate::wav::{self, Dither, WavSpec, WavWriter};

fn fail(message: impl AsRef<str>) -> SampleError {
    SampleError::Invalid(format!("Audio delivery: {}", message.as_ref()))
}

/// The format of a delivered file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliverySettings {
    format: ExportFormat,
}

impl DeliverySettings {
    /// `encoding` at `sample_rate` (44 100, 48 000 or 96 000 Hz), from a 48 kHz render, without
    /// dither.
    pub fn new(encoding: ExportEncoding, sample_rate: u32) -> Result<Self> {
        Self::from_format(ExportFormat { encoding, sample_rate, dither: Dither::None, render_sample_rate: RENDER_SAMPLE_RATE })
    }

    /// The settings of a validated export format.
    pub fn from_format(format: ExportFormat) -> Result<Self> {
        if !SUPPORTED_SAMPLE_RATES.contains(&format.sample_rate) && format.sample_rate != format.render_sample_rate {
            return Err(fail("Choose 44.1, 48 or 96 kHz."));
        }
        if !crate::resample::SAMPLE_RATE_RANGE.contains(&format.render_sample_rate) {
            return Err(fail("Unsupported render sample rate."));
        }
        Ok(DeliverySettings { format })
    }

    /// The same settings with TPDF dither (or none) for the integer encodings.
    pub fn with_dither(mut self, dither: Dither) -> Self {
        self.format.dither = if self.format.encoding.is_float() { Dither::None } else { dither };
        self
    }

    /// The export format these settings deliver.
    pub fn format(&self) -> ExportFormat {
        self.format
    }

    /// Sample encoding.
    pub fn encoding(&self) -> ExportEncoding {
        self.format.encoding
    }

    /// Delivered sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.format.sample_rate
    }

    /// The description that accompanies the delivered file.
    pub fn metadata(&self) -> ExportMetadata {
        self.format.metadata()
    }
}

/// Frames a delivery at `sample_rate` holds for `input_frames` frames of a render at
/// `render_sample_rate`: the exact ceiling of the end point.
///
/// When a render is split (music, then a tail), convert the absolute end points and subtract;
/// rounding each piece separately would drift.
pub fn output_frames(input_frames: u64, render_sample_rate: u32, sample_rate: u32) -> Result<u64> {
    if render_sample_rate == 0 || sample_rate == 0 {
        return Err(fail("Unsupported delivery sample rate."));
    }
    resampled_frames(input_frames, render_sample_rate, sample_rate)
        .filter(|&frames| frames <= i64::MAX as u64)
        .ok_or_else(|| fail("Delivery frame count exceeds its integer range."))
}

/// Checks that a delivery fits in a WAV file (RIFF sizes are 32-bit; RF64 is not written) and
/// returns its exact frame count.
pub fn preflight(input_frames: u64, settings: &DeliverySettings) -> Result<u64> {
    if input_frames == 0 {
        return Err(fail("The source file is empty."));
    }
    let count = output_frames(input_frames, settings.format.render_sample_rate, settings.sample_rate())?;
    let size = count.checked_mul(2 * settings.encoding().bit_depth() as u64 / 8);
    if size.is_none_or(|size| size > u32::MAX as u64 - 4096) {
        return Err(fail("The requested WAV exceeds the supported RIFF size. Export a shorter range or smaller delivery format."));
    }
    Ok(count)
}

/// Where a delivery job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryProgress {
    /// Render frames read.
    pub input_frames: u64,
    /// Delivery frames written.
    pub output_frames: u64,
    /// Delivery frames the finished file will have.
    pub total_output_frames: u64,
    /// True once the file is complete and verified.
    pub complete: bool,
}

/// What a finished delivery produced.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryResult {
    /// Render frames consumed.
    pub input_frames: u64,
    /// Frames in the delivered file.
    pub output_frames: u64,
    /// The file's format.
    #[serde(flatten)]
    pub metadata: ExportMetadata,
}

/// A resumable conversion of one render file into one WAV file.
///
/// The WAV is written into a private folder next to the destination and moved to the
/// destination by [`DeliveryJob::publish`], which never replaces an existing file. A job that
/// fails, is discarded or is dropped removes its private folder and leaves no file. The
/// destination is normally an [`super::ExportEntry::delivery_path`]; the surrounding
/// [`super::ExportTransaction`] owns the replacement of whatever the user chose.
pub struct DeliveryJob {
    destination: PathBuf,
    settings: DeliverySettings,
    expected_input_frames: u64,
    expected_output_frames: u64,
    owned_directory: PathBuf,
    staging: PathBuf,
    reader: Option<RenderFileReader>,
    writer: Option<WavWriter<BufWriter<File>>>,
    resampler: Option<Resampler>,
    block_frames: usize,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    input_frames: u64,
    written_frames: u64,
    result: Option<DeliveryResult>,
    published: bool,
    inactive: bool,
}

impl DeliveryJob {
    /// Prepares the conversion of `source` (a stereo float render file with exactly
    /// `expected_input_frames` frames at the settings' render rate) to `destination` (a `.wav`
    /// path that does not exist yet, in an existing folder).
    ///
    /// `block_frames` (1...65 536) is how many render frames one step reads; it affects only
    /// how often the caller regains control, never the audio.
    pub fn new(source: &Path, expected_input_frames: u64, destination: &Path, settings: DeliverySettings, block_frames: usize) -> Result<Self> {
        let is_wav = destination.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        if source == destination || !is_wav || !(1..=65_536).contains(&block_frames) {
            return Err(fail("Invalid delivery path or block capacity."));
        }
        let parent = destination.parent().filter(|parent| parent.is_dir());
        let Some(parent) = parent.filter(|_| destination.symlink_metadata().is_err()) else {
            return Err(fail("The private delivery destination must be absent in an existing directory."));
        };
        let expected_output_frames = preflight(expected_input_frames, &settings)?;
        let reader = RenderFileReader::open(source).map_err(|error| fail(error.to_string()))?;
        let render_rate = settings.format.render_sample_rate;
        let is_caf = source.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("caf"));
        if !is_caf || reader.frames() != expected_input_frames || reader.sample_rate() != render_rate || reader.channels() != 2 {
            return Err(fail(format!("Expected the complete stereo Float32/{render_rate} Hz CAF render with its exact admitted frame count.")));
        }
        let resampler = if settings.sample_rate() != render_rate {
            Some(Resampler::new(render_rate, settings.sample_rate(), 2, ResampleQuality::Mastering).map_err(|error| fail(error.to_string()))?)
        } else {
            // A same-rate delivery is copied: no converter, normalisation or limiter in the way.
            None
        };
        let owned_directory = parent.join(format!(".delivery-{}", unique_token()));
        std::fs::create_dir(&owned_directory).map_err(|e| SampleError::io("Create the private delivery folder", &e))?;
        let staging = owned_directory.join("audio.wav");
        let spec = WavSpec { sample_rate: settings.sample_rate(), channels: 2, encoding: settings.encoding().wav_encoding(), dither: settings.format.dither };
        let writer = match WavWriter::create_new(&staging, spec, &[]) {
            Ok(writer) => writer,
            Err(error) => {
                std::fs::remove_dir_all(&owned_directory).ok();
                return Err(error);
            }
        };
        Ok(DeliveryJob {
            destination: destination.to_path_buf(),
            settings,
            expected_input_frames,
            expected_output_frames,
            owned_directory,
            staging,
            reader: Some(reader),
            writer: Some(writer),
            resampler,
            block_frames,
            input: vec![Vec::new(), Vec::new()],
            output: vec![Vec::new(), Vec::new()],
            input_frames: 0,
            written_frames: 0,
            result: None,
            published: false,
            inactive: false,
        })
    }

    /// The path the finished file is published to.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// The format being delivered.
    pub fn settings(&self) -> &DeliverySettings {
        &self.settings
    }

    /// Frames the finished file will have.
    pub fn expected_output_frames(&self) -> u64 {
        self.expected_output_frames
    }

    /// The outcome, once [`DeliveryJob::advance`] has completed the file.
    pub fn result(&self) -> Option<&DeliveryResult> {
        self.result.as_ref()
    }

    /// True after a successful [`DeliveryJob::publish`].
    pub fn is_published(&self) -> bool {
        self.published
    }

    /// Current counters.
    pub fn progress(&self) -> DeliveryProgress {
        DeliveryProgress {
            input_frames: self.input_frames,
            output_frames: self.written_frames,
            total_output_frames: self.expected_output_frames,
            complete: self.result.is_some(),
        }
    }

    /// Converts up to `maximum_blocks` (1...256) blocks and returns the progress. Call it until
    /// `complete`; then [`DeliveryJob::publish`]. Any error discards the job.
    pub fn advance(&mut self, maximum_blocks: usize) -> Result<DeliveryProgress> {
        if !(1..=256).contains(&maximum_blocks) || self.inactive {
            return Err(fail("The delivery job is inactive or its work budget is invalid."));
        }
        if self.result.is_some() {
            return Ok(self.progress());
        }
        match self.run(maximum_blocks) {
            Ok(()) => Ok(self.progress()),
            Err(error) => {
                self.discard();
                Err(error)
            }
        }
    }

    fn run(&mut self, maximum_blocks: usize) -> Result<()> {
        for _ in 0..maximum_blocks {
            if self.input_frames < self.expected_input_frames {
                let reader = self.reader.as_mut().ok_or_else(|| fail("Source input is unavailable."))?;
                let wanted = (self.expected_input_frames - self.input_frames).min(self.block_frames as u64) as usize;
                let count = reader.read_planar(wanted, &mut self.input).map_err(|error| fail(error.to_string()))?;
                if count != wanted {
                    return Err(fail("The intermediate render changed or ended early."));
                }
                require_finite(&self.input)?;
                self.input_frames += count as u64;
                match self.resampler.as_mut() {
                    Some(resampler) => {
                        self.output.iter_mut().for_each(Vec::clear);
                        resampler.process(&self.input, &mut self.output);
                    }
                    None => std::mem::swap(&mut self.input, &mut self.output),
                }
                self.write_output()?;
            }
            if self.input_frames == self.expected_input_frames {
                if let Some(resampler) = self.resampler.as_mut() {
                    self.output.iter_mut().for_each(Vec::clear);
                    resampler.finish(&mut self.output);
                    self.write_output()?;
                }
                return self.finish();
            }
        }
        Ok(())
    }

    fn write_output(&mut self) -> Result<()> {
        require_finite(&self.output)?;
        let available = self.output[0].len() as u64;
        if self.written_frames + available > self.expected_output_frames {
            return Err(fail("Delivery output exceeded its admitted frame count."));
        }
        if available > 0 {
            let writer = self.writer.as_mut().ok_or_else(|| fail("The delivery writer is unavailable."))?;
            writer.write_planar(&self.output)?;
            self.written_frames += available;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if self.written_frames != self.expected_output_frames {
            return Err(fail(format!(
                "The converter did not supply the exact admitted output duration ({}/{} frames).",
                self.written_frames, self.expected_output_frames
            )));
        }
        self.reader = None;
        self.resampler = None;
        let writer = self.writer.take().ok_or_else(|| fail("The delivery writer is unavailable."))?;
        let (out, _) = writer.finalize()?;
        let file = out.into_inner().map_err(|e| SampleError::io("Finish audio file", e.error()))?;
        file.sync_all().map_err(|e| SampleError::io("Finish audio file", &e))?;
        drop(file);
        // Trust the bytes on disk, not the writer's bookkeeping.
        let header = wav::read_header(&self.staging).ok_or_else(|| fail("The completed delivery is not a RIFF/WAVE file."))?;
        let encoding = self.settings.encoding();
        let matches = header.frames == self.expected_output_frames
            && header.sample_rate == self.settings.sample_rate()
            && header.channels == 2
            && header.bits_per_sample as u32 == encoding.bit_depth()
            && header.is_float() == encoding.is_float()
            && (header.is_float() || header.format_tag == 1);
        if !matches {
            return Err(fail("The completed WAV header does not match its delivery settings."));
        }
        self.result = Some(DeliveryResult { input_frames: self.expected_input_frames, output_frames: self.expected_output_frames, metadata: self.settings.metadata() });
        Ok(())
    }

    /// Moves the finished file to the destination. Fails, leaving any file that appeared there
    /// meanwhile untouched, if the destination is no longer free.
    pub fn publish(&mut self) -> Result<DeliveryResult> {
        let Some(result) = self.result.clone().filter(|_| !self.inactive && !self.published) else {
            return Err(fail("Delivery must finish and verify before publication."));
        };
        fsx::rename_exclusive(&self.staging, &self.destination)
            .map_err(|error| SampleError::Io(format!("Cannot publish the private delivery WAV without replacing another file: {error}.")))?;
        self.published = true;
        self.inactive = true;
        // A successful publication is not undone by failing to remove the now-empty folder.
        std::fs::remove_dir_all(&self.owned_directory).ok();
        Ok(result)
    }

    /// Cancels the job: closes its files and removes its private folder. A published file
    /// belongs to the caller and stays.
    pub fn discard(&mut self) {
        self.reader = None;
        self.writer = None;
        self.resampler = None;
        self.inactive = true;
        std::fs::remove_dir_all(&self.owned_directory).ok();
    }
}

impl Drop for DeliveryJob {
    fn drop(&mut self) {
        self.discard();
    }
}

fn require_finite(channels: &[Vec<f32>]) -> Result<()> {
    if channels.iter().all(|channel| channel.iter().all(|value| value.is_finite())) {
        Ok(())
    } else {
        Err(fail("Non-finite audio cannot be delivered."))
    }
}

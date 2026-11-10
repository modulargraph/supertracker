//! Audio files for SuperTracker: everything that needs neither an audio device nor the engine.
//! See docs/RUST_ARCHITECTURE.md.
//!
//! | Area | Module | Entry points |
//! |---|---|---|
//! | Decoding to float PCM | [`decode`] | [`decode_file`], [`decode_planar`], [`AudioFileReader`] |
//! | Sample summary and waveform | [`pcm`], [`inspect`] | [`DecodedSample::info`], [`DecodedSample::inspect`] |
//! | Sample-rate conversion | [`resample`] | [`Resampler`], [`resample::resample`] |
//! | Sample edits | [`edit`] | [`edit_to_file`], [`apply_edit`] |
//! | Time stretch | [`stretch`] | [`stretch::stretch`] |
//! | Transient detection | [`slicing`] | [`analyze_slices`] |
//! | Wavetables and built-in samples | [`wavetable`], [`builtin`], [`riff`] | [`load_sample`], [`builtin::wavetable_list`] |
//! | WAV writing | [`wav`] | [`wav::WavWriter`] |
//! | Export | [`export`] | [`export::ExportFormat`], [`export::DeliveryJob`], [`export::ExportTransaction`] |
//! | App-owned file locations | [`paths`] | [`paths::recording_destination`], [`paths::edit_directory`] |
//! | Browser preview decoding | [`preview`] | [`preview::PreviewCache`], [`preview::PreviewStream`] |
//!
//! Conventions: errors are [`SampleError`] with messages a user can read; long operations take
//! an [`Operation`] (cancellation token and progress callback); request and result types that
//! cross the bridge derive `serde` traits with the field names the interface uses.
//!
//! Nothing here is meant to run on the audio thread except the readers of
//! [`preview::PreviewPcm`] and [`preview::PreviewStreamReader`], which are documented as such.
//!
//! Platform notes: the export transaction ([`export::ExportTransaction`]) uses directory-relative
//! POSIX calls and exists on Unix only. Its exclusive rename is `renameat2` on Linux (exercised
//! by the tests) and `renamex_np` on macOS (type-checked for `aarch64-apple-darwin`, not yet run
//! there).

#![warn(missing_docs)]

pub mod builtin;
pub mod decode;
pub mod edit;
pub mod error;
pub mod export;
pub mod inspect;
mod mp4;
pub mod op;
pub mod paths;
pub mod pcm;
pub mod preview;
pub mod resample;
pub mod riff;
pub mod slicing;
pub mod stretch;
pub mod wav;
pub mod wavetable;

use std::path::Path;

pub use decode::{decode_file, decode_planar, probe_file, AudioFileReader, DecodeOptions, SourceSpec};
pub use edit::{apply_edit, edit_to_file, EditRequest};
pub use error::{Result, SampleError};
pub use inspect::{InspectRequest, Inspection};
pub use op::{CancelToken, Operation, Progress};
pub use pcm::{DecodedSample, PlanarPcm, SampleInfo, MAX_SAMPLE_FRAMES};
pub use resample::{ResampleQuality, Resampler};
pub use slicing::{analyze_slices, SliceAnalysis, SliceRequest};

/// Loads the sample a project reference names: a `builtin:` reference is generated (drum hits
/// through `drum_renderer`, which the caller that owns the engine supplies), anything else is
/// decoded from the file at that path.
pub fn load_sample(reference: &str, drum_renderer: Option<builtin::DrumRenderer<'_>>, operation: &mut Operation<'_>) -> Result<DecodedSample> {
    if builtin::is_builtin(reference) {
        builtin::load_builtin(reference, drum_renderer)
    } else {
        decode_file(reference, Path::new(reference), operation)
    }
}

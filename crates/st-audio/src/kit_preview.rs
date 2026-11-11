//! The audio preview a kit is saved with: what the Kits browser plays when a kit is auditioned.
//!
//! A kit is a group of tracks with its own group bus. Its preview is that bus as it sounds in
//! the pattern the session has selected: the pattern is rendered once from its first row on a
//! private engine ([`crate::offline`]; the live engine keeps playing), and the group bus is
//! captured after its fader and before its output, so the preview holds the kit with its
//! members' instruments and inserts (built-in effects, and plug-ins in containers of the
//! render's own) and the bus's own processing, and nothing of the rest of the session.
//!
//! | | |
//! |---|---|
//! | Length | the pattern, but at most [`MAXIMUM_BARS`] bars and [`MAXIMUM_SECONDS`] seconds, plus [`TAIL_SECONDS`] of tail (the transport stops at the musical end, held notes are released, effects ring out) |
//! | File | WAV, stereo, 16-bit PCM with TPDF dither, 48 kHz (`Preview.wav` in the kit package; see `st_library::kit`) |
//!
//! A preview needs something to play: a pattern in which no member of the kit has a note is
//! refused with a message that says so, as is a kit whose notes all go to external MIDI
//! instruments (an offline render cannot hear a device). When only some members are external
//! the preview is rendered without them and [`KitPreview::warning`] says so.

use crate::export::{deliver, offline_engine, require_renderable, FileTarget, RenderContext};
use crate::offline::RunSummary;
use serde_json::{json, Value};
use st_project::model::{InstrumentKind, Project};
use st_samples::export::format::RENDER_SAMPLE_RATE;
use st_samples::export::{DeliverySettings, ExportFormat, ExportFormatRequest, ExportKind, ExportTransaction};
use std::path::Path;

/// The longest preview, without its tail, in seconds.
pub const MAXIMUM_SECONDS: f64 = 10.0;
/// The longest preview, without its tail, in bars of the session's meter and tempo.
pub const MAXIMUM_BARS: f64 = 8.0;
/// What follows the musical end, in seconds.
pub const TAIL_SECONDS: f64 = 1.0;
/// The sample rate of a preview file.
pub const SAMPLE_RATE: u32 = 48_000;

const CANCELLED: &str = "The kit preview was cancelled.";

/// What [`render`] wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct KitPreview {
    /// The file.
    pub path: String,
    /// The channel that was captured: the kit's group bus.
    pub owner: String,
    /// The number of the pattern that was played.
    pub pattern: i64,
    /// Frames in the file, and how many of them are the pattern (the rest is the tail).
    pub frames: u64,
    pub musical_frames: u64,
    pub sample_rate: u32,
    /// Something the user should know about the preview (members that could not be rendered).
    pub warning: Option<String>,
}

/// What the kit's members play in the pattern.
struct Notes {
    /// A member plays a note with an instrument the engine or a plug-in sounds.
    audible: bool,
    /// A member plays a note with an external MIDI instrument.
    external: bool,
}

fn notes_of(project: &Project, group: &str, pattern: i64) -> Notes {
    let mut notes = Notes { audible: false, external: false };
    let Some(pattern) = project.pattern_numbered(pattern) else { return notes };
    let instruments = project.instrument_list();
    for (index, track) in project.track_list().iter().enumerate().filter(|(_, track)| track.group_id.as_deref() == Some(group)) {
        let rows = pattern.steps.as_ref().and_then(|steps| steps.get(index)).map(Vec::as_slice).unwrap_or_default();
        for step in rows.iter().flatten() {
            let cells = std::iter::once((step.note(), step.instrument())).chain(step.columns.iter().flatten().flatten().map(|cell| (cell.note(), cell.instrument())));
            for (note, instrument) in cells {
                if !(0..=127).contains(&note) {
                    continue;
                }
                let instrument = if instrument >= 0 { instrument } else { track.instrument() };
                let external = instruments.get(instrument.max(0) as usize).is_some_and(|instrument| instrument.kind() == InstrumentKind::Midi);
                notes.audible |= !external;
                notes.external |= external;
            }
        }
    }
    notes
}

/// Renders the preview of the kit `group` of `project` (the session as the page publishes it,
/// plug-in states included) into the WAV file `destination`. An existing file there is
/// replaced; on an error the destination is as it was.
#[cfg(unix)]
pub fn render(context: &RenderContext<'_>, project: &Value, group: &str, destination: &Path) -> Result<KitPreview, String> {
    require_renderable(project)?;
    let parsed = Project::from_value(project.clone()).map_err(|error| error.to_string())?;
    let bus = parsed.groups.iter().flatten().find(|item| item.id.as_deref() == Some(group)).ok_or("Choose an existing kit to preview.")?.bus_id.clone().filter(|bus| !bus.is_empty());
    let owner = format!("bus:{}", bus.ok_or("This kit has no group bus to take a preview from.")?);
    let pattern = parsed.active_pattern();
    if parsed.pattern_numbered(pattern).is_none() {
        return Err("The selected pattern does not exist.".to_string());
    }
    let notes = notes_of(&parsed, group, pattern);
    if !notes.audible {
        return Err(if notes.external {
            "this kit plays external MIDI instruments only in the selected pattern. Record their audio into its tracks to get a preview."
        } else {
            "the kit plays no note in the selected pattern. Add a note for one of its pads and save the kit again."
        }
        .to_string());
    }

    // The pattern as a one-entry song, without the timeline's clips, played once.
    let mut session = project.clone();
    session["sequence"] = json!([pattern]);
    session["audioClips"] = json!([]);
    session["loopSong"] = json!(false);
    session["metronome"] = json!(false);
    let offline = offline_engine(context, &session)?;
    if !offline.spec().strips.iter().any(|strip| strip.id == owner) {
        return Err("The kit's group bus is not part of the mixer.".to_string());
    }
    let rate = offline.sample_rate();
    let duration = offline.engine().duration(true);
    if !(duration > 0.0 && duration.is_finite()) {
        return Err("The selected pattern has no length.".to_string());
    }
    let (numerator, denominator) = parsed.meter();
    let bar_seconds = numerator.clamp(1, 64) as f64 * 4.0 / denominator.clamp(1, 64) as f64 * 60.0 / parsed.bpm().clamp(20.0, 999.0);
    let musical_frames = ((duration.min(MAXIMUM_SECONDS).min(MAXIMUM_BARS * bar_seconds) * rate) as u64).max(1);
    let frames = musical_frames + (TAIL_SECONDS * rate) as u64;

    let request = ExportFormatRequest { encoding: Some("pcm16".to_string()), sample_rate: Some(SAMPLE_RATE as f64), ..ExportFormatRequest::default() };
    let format = ExportFormat::resolve(&request, "pattern", RENDER_SAMPLE_RATE).map_err(|error| error.to_string())?;
    let settings = DeliverySettings::from_format(format).map_err(|error| error.to_string())?;
    let delivered = st_samples::export::delivery::preflight(frames, &settings).map_err(|error| error.to_string())?;
    let delivered_musical = st_samples::export::delivery::output_frames(musical_frames, RENDER_SAMPLE_RATE, settings.sample_rate()).map_err(|error| error.to_string())?;
    let mut transaction = ExportTransaction::new(destination, ExportKind::File).map_err(|error| error.to_string())?;
    let leaf = destination.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "Preview.wav".to_string());
    let entry = transaction.register(&leaf).map_err(|error| error.to_string())?;
    let outcome = (|| -> Result<RunSummary, String> {
        offline.play_from(pattern as i32, 0, 0, true);
        let rendering = |fraction: f64| (context.progress)(0.8 * fraction, "rendering");
        let mut target = FileTarget::new(&[entry.source_path.as_path()], RENDER_SAMPLE_RATE, frames, context.cancel, CANCELLED, &rendering)?;
        let summary = offline.run(&[Some(owner.clone())], Some(musical_frames), &mut target)?;
        target.finish()?;
        deliver(std::slice::from_ref(&entry), frames, settings, context.cancel, CANCELLED, &|fraction| (context.progress)(0.8 + 0.2 * fraction, "delivery"))?;
        Ok(summary)
    })();
    if let Err(error) = outcome {
        let _ = transaction.discard();
        return Err(error);
    }
    let paths = transaction.publish().map_err(|error| error.to_string())?;
    Ok(KitPreview {
        path: paths.first().map(|path| path.to_string_lossy().into_owned()).unwrap_or_else(|| destination.to_string_lossy().into_owned()),
        owner,
        pattern,
        frames: delivered,
        musical_frames: delivered_musical.min(delivered),
        sample_rate: settings.sample_rate(),
        warning: notes.external.then(|| "The kit's external MIDI instruments are not in its audio preview. Record their audio into its tracks to hear them there.".to_string()),
    })
}

/// Offline rendering to files is not available on this system.
#[cfg(not(unix))]
pub fn render(_context: &RenderContext<'_>, _project: &Value, _group: &str, _destination: &Path) -> Result<KitPreview, String> {
    Err("kit previews cannot be rendered on this system yet.".to_string())
}

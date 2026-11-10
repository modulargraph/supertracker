//! Standard MIDI File import and export (SMF type 0 and 1, PPQN timing).
//!
//! Behaviour is documented in `docs/MIDI_FILES.md`. In short:
//!
//! * [`import`] turns a `.mid` file into a new project at four rows per beat: sub-row onsets
//!   become microtiming effects, equal simultaneous notes become chords, overlapping notes go
//!   into extra note columns and then extra tracks, CC 7/10/91 become volume, pan and reverb
//!   automation, a changing tempo is flattened to one tempo while elapsed time is kept. What the
//!   tracker cannot represent is counted in `project.midiImport.warnings`, never dropped silently.
//! * [`export`] writes the song sequence once as SMF type 1 at 960 ticks per quarter note,
//!   respecting mute and solo, with swing, microtiming, ratchets, chords, note columns, gates
//!   (and gate-less notes that last until the next note), program and bank selection, and
//!   volume/pan/reverb or explicitly mapped automation as controllers.
//!
//! Format reference: <https://midi.org/standard-midi-files-specification>.

mod export;
mod import;

pub use export::{export, export_file, MidiExport};
pub use import::{import, import_file};

/// MIDI files larger than this are not read, and larger exports are not written (64 MiB).
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Events a file may contain.
pub const MAX_EVENTS: usize = 1_000_000;
/// Rows an import can fill: 256 patterns of 256 rows.
pub const MAX_IMPORT_ROWS: usize = 65_536;
/// Ticks per quarter note of exported files.
pub const EXPORT_PPQN: u16 = 960;

pub(crate) fn clamp(value: f64, low: f64, high: f64) -> f64 {
    high.min(low.max(value))
}

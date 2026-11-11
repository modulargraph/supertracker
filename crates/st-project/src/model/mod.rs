//! The typed project document.
//!
//! This mirrors the JSON the interface publishes (`ui/app-project-model.js` and the
//! modules around it). The interface owns the document; the host reads it, validates it, stores
//! it and hands it back. Three rules shape every type here:
//!
//! * **Every field is optional** (`Option<T>`): a field that is absent in the JSON stays absent
//!   when written back. A method of the same name returns the value or the documented default,
//!   which is the default the interface and the audio host use (`track.volume` is the stored
//!   value, `track.volume()` is what plays).
//! * **Nothing is dropped**: each object keeps unknown keys, `null`s and unreadable values in
//!   its `extra` map and writes them back (see [`crate::json`]).
//! * **One format**: a project says which format it is in with `version`, and
//!   [`crate::validate::validate`] refuses every number but [`FORMAT_VERSION`]. Nothing is
//!   converted.
//!
//! Media paths (samples, clips, prints) are absolute while a project is open and relative to the
//! project file on disk; [`crate::media`] knows which fields those are.

pub mod clip;
pub mod instrument;
pub mod pattern;
pub mod plugin;
pub mod routing;

pub use clip::{AudioClip, FreezeRecord, PatternPrint, PrintLoop, Take, TakeSet};
pub use instrument::{FilterType, Instrument, InstrumentKind, LfoShape, LfoTarget, SamplerMode, VoiceMode, Zone};
pub use pattern::{AutomationLane, AutomationPoint, AutomationTarget, ColumnCell, Interpolation, Pattern, Step, StepEffect, StepEffectKind};
pub use plugin::{Owner, ParameterPage, PluginDescriptor, PluginKind, PluginPages};
pub use routing::{Bus, BusKind, LayoutSignature, Routing, Send, Sidechain, SidechainDestination, Tap};

use crate::error::{Error, Result};
use crate::json::{model, Json};
use serde_json::Value;
use std::collections::BTreeMap;

/// The project format this crate reads and writes (`project.version`). It changes whenever a
/// project written before the change would no longer mean the same, or be refused, after it.
/// `PROJECT_VERSION` in `ui/app-project-model.js` is the same number.
pub const FORMAT_VERSION: i64 = 2;

model! {
    /// A track's live audio input (`line-inputs.js`): channels of an audio device mixed into
    /// the track's mixer channel ahead of its inserts. This is the whole routing: the host
    /// derives what it opens and mixes from it at every publication.
    pub struct AudioInput {
        /// Whether the input feeds the track.
        "enabled" => enabled: bool = false,
        /// The audio device the channels were chosen on (the `uid` the host lists it with).
        /// The input is live only while that device is the host's audio input.
        "deviceUID" => device_uid: String,
        /// Input channels of the device, from 0: one for mono (heard on both sides), two for
        /// stereo.
        "channels" => channels: Vec<i64>,
        /// Input gain, 0..2.
        "gain" => gain: f64 = 1.0,
        /// Whether the input is heard through the track. Absent: it is. Off keeps the route
        /// (and what it names) but mixes nothing into the track.
        "monitor" => monitor: bool = true,
    }
}

model! {
    /// A track: one column of every pattern and one mixer channel.
    pub struct Track {
        /// The track's index, as the interface numbers it.
        "id" => id: i64,
        /// Display name.
        "name" => name: String,
        /// Colour (`#rrggbb`).
        "color" => color: String,
        /// Fader, 0..2.
        "volume" => volume: f64 = 0.8,
        /// Pan, -1..1.
        "pan" => pan: f64 = 0.0,
        /// Muted.
        "mute" => mute: bool = false,
        /// Soloed.
        "solo" => solo: bool = false,
        /// The instrument the track plays (index into `project.instruments`); steps may name another.
        "instrument" => instrument: i64 = 0,
        /// Send to the shared delay, 0..1.
        "delaySend" => delay_send: f64 = 0.0,
        /// Send to the shared reverb, 0..1.
        "reverbSend" => reverb_send: f64 = 0.0,
        /// Stable identity of the track's mixer channel: its owner name is `track:<routingID>`.
        /// Required.
        "routingID" => routing_id: String,
        /// The track group the track belongs to (`project.groups[].id`). Members are contiguous.
        "groupID" => group_id: String,
        /// Kit pad position within the group, 0..15.
        "pad" => pad: i64,
        /// The note pad keys enter on this track.
        "padNote" => pad_note: i64 = 60,
        /// Choke group within the track's group, 0 = off, else 1..16.
        "chokeGroup" => choke_group: i64 = 0,
        /// Overrides the instrument's voice mode.
        "voiceMode" => voice_mode: VoiceMode,
        /// Note columns, 1..8. Absent for one.
        "noteColumns" => note_columns: i64 = 1,
        /// MIDI channel (1..16) notes are exported on when the instrument names none.
        "midiChannel" => midi_channel: i64,
        /// Live audio input.
        "audioInput" => audio_input: AudioInput,
        /// Track Freeze record while the track is frozen.
        "frozen" => frozen: FreezeRecord,
    }
}

impl Track {
    /// The name shown for the track at `index`.
    pub fn display_name(&self, index: usize) -> String {
        match self.name.as_deref() {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => format!("Track {}", index + 1),
        }
    }

    /// The track's mixer channel name, `track:<routingID>`, when it has a routing identity.
    pub fn owner(&self) -> Option<String> {
        self.routing_id.as_deref().filter(|id| !id.is_empty()).map(|id| format!("track:{id}"))
    }

    /// Note columns in effect, 1..=8.
    pub fn note_column_count(&self) -> usize {
        self.note_columns().clamp(1, pattern::MAX_NOTE_COLUMNS) as usize
    }
}

model! {
    /// A track group: a contiguous run of tracks routed to one group bus. Kits load as groups.
    pub struct Group {
        /// Identity; member tracks carry it as `groupID`.
        "id" => id: String,
        /// Display name.
        "name" => name: String,
        /// Colour.
        "color" => color: String,
        /// Collapsed in the pattern editor.
        "collapsed" => collapsed: bool = false,
        /// Collapsed in the mixer.
        "mixerCollapsed" => mixer_collapsed: bool = false,
        /// The group's bus (`routing.buses[].id`).
        "busID" => bus_id: String,
        /// The kit file the group was loaded from.
        "libraryPath" => library_path: String,
        /// Notes from the import that created the group.
        "importNotes" => import_notes: Vec<String>,
    }
}

model! {
    /// The project's meter.
    pub struct TimeSignature {
        /// Beats per bar, 1..16.
        "numerator" => numerator: i64 = 4,
        /// Beat unit: 2, 4, 8 or 16.
        "denominator" => denominator: i64 = 4,
    }
}

model! {
    /// A section of the song that loops while `loopSong` is on: inclusive positions in `sequence`.
    pub struct SongLoop {
        /// First sequence position.
        "start" => start: i64,
        /// Last sequence position.
        "end" => end: i64,
        /// The interface's note of `loopSong` before the section was set.
        "restoreLoopSong" => restore_loop_song: bool,
    }
}

model! {
    /// The shared delay return (`send-effects.js`).
    pub struct Delay {
        /// Delay time in seconds when not synced.
        "time" => time: f64 = 0.375,
        /// Feedback, 0..1.
        "feedback" => feedback: f64 = 0.35,
        /// Return level, 0..1.
        "mix" => mix: f64 = 0.3,
        /// Tempo-synced time in quarter notes; 0 = free time.
        "beats" => beats: f64 = 0.0,
        /// Tone, 0..1 (1 = open).
        "tone" => tone: f64 = 1.0,
        /// The division label the interface shows for `beats`; `null` for free time.
        "sync" => sync: String,
    }
}

model! {
    /// The shared reverb return.
    pub struct Reverb {
        /// Room size, 0..1.
        "size" => size: f64 = 0.7,
        /// Damping, 0..1.
        "damping" => damping: f64 = 0.4,
        /// Return level, 0..1.
        "mix" => mix: f64 = 0.25,
        /// Pre-delay in seconds.
        "predelay" => predelay: f64 = 0.0,
    }
}

model! {
    /// View preferences saved with the project (never an Undo step).
    pub struct ViewState {
        /// The pattern editor shows the Gate column.
        "gateColumn" => gate_column: bool = false,
        /// The automation panel's scope when shown: `pattern` or `song`.
        "automationPanel" => automation_panel: String,
        /// The song view follows playback.
        "songScroll" => song_scroll: bool = false,
        /// Empty tracks are hidden.
        "hideEmpty" => hide_empty: bool = false,
        /// What "empty" looks at: `pattern` or `song`.
        "hideEmptyScope" => hide_empty_scope: String,
        /// Routing identities of collapsed tracks.
        "collapsedTracks" => collapsed_tracks: Vec<String>,
    }
}

model! {
    /// A tempo event of an imported MIDI file.
    pub struct MidiTempoChange {
        /// Position in the file's ticks.
        "tick" => tick: i64,
        /// Tempo as microseconds per quarter note.
        "microsecondsPerQuarter" => microseconds_per_quarter: i64,
    }
}

model! {
    /// A time signature event of an imported MIDI file.
    pub struct MidiTimeSignature {
        /// Position in ticks.
        "tick" => tick: i64,
        /// Beats per bar.
        "numerator" => numerator: i64,
        /// Beat unit.
        "denominator" => denominator: i64,
    }
}

model! {
    /// A program change of an imported MIDI file.
    pub struct MidiProgramChange {
        /// Position in ticks.
        "tick" => tick: i64,
        /// Channel, 1..16.
        "channel" => channel: i64,
        /// Program, 0..127.
        "program" => program: i64,
    }
}

model! {
    /// A controller event of an imported MIDI file.
    pub struct MidiControllerEvent {
        /// Position in ticks.
        "tick" => tick: i64,
        /// Channel, 1..16.
        "channel" => channel: i64,
        /// Controller number.
        "controller" => controller: i64,
        /// Value, 0..127.
        "value" => value: i64,
    }
}

model! {
    /// What a MIDI import kept about its source (`project.midiImport`, see `docs/MIDI_FILES.md`).
    pub struct MidiImportInfo {
        /// The imported file.
        "source" => source: String,
        /// SMF type, 0 or 1.
        "format" => format: i64,
        /// The file's ticks per quarter note.
        "ticksPerQuarter" => ticks_per_quarter: i64,
        /// What the import could not represent.
        "warnings" => warnings: Vec<String>,
        /// Notes placed in patterns.
        "importedNotes" => imported_notes: i64,
        /// Notes in the file.
        "sourceNotes" => source_notes: i64,
        /// The file's tempo map.
        "tempoChanges" => tempo_changes: Vec<MidiTempoChange>,
        /// The file's meter changes.
        "timeSignatures" => time_signatures: Vec<MidiTimeSignature>,
        /// The file's program changes (first 100,000).
        "programChanges" => program_changes: Vec<MidiProgramChange>,
        /// The file's controller events (first 100,000).
        "controllers" => controllers: Vec<MidiControllerEvent>,
    }
}

model! {
    /// A recorded content fingerprint of a media file (`project.mediaManifest`), so a missing
    /// file can be recognised by content when it is found again.
    pub struct MediaManifestEntry {
        /// The file (media path, rewritten together with the references to it).
        "path" => path: String,
        /// Size in bytes.
        "size" => size: i64,
        /// Modification time, seconds since 1970.
        "modified" => modified: f64,
        /// SHA-256 of the content, 64 hexadecimal digits.
        "sha256" => sha256: String,
    }
}

model! {
    /// One replacement made by a relink.
    pub struct RelinkChange {
        /// The reference that changed (see [`crate::relink::Reference::id`]).
        "id" => id: String,
        /// The path before.
        "from" => from: String,
        /// The path after.
        "to" => to: String,
        /// The replacement's content differs from the recorded fingerprint (chosen deliberately).
        "differentContent" => different_content: bool = false,
    }
}

model! {
    /// One relink operation (`project.mediaRelinkHistory`, the last 64 are kept).
    pub struct RelinkRecord {
        /// When (ISO 8601, UTC).
        "date" => date: String,
        /// What changed.
        "changes" => changes: Vec<RelinkChange>,
    }
}

model! {
    /// The project document.
    pub struct Project {
        /// Format version: [`FORMAT_VERSION`]. Required; a project with another number is refused.
        "version" => version: i64,
        /// Session name.
        "name" => name: String,
        /// Tempo, 20..400.
        "bpm" => bpm: f64 = 120.0,
        /// Pattern rows per quarter note.
        "linesPerBeat" => lines_per_beat: i64 = 4,
        /// Swing, 0..0.95: odd rows start later by this share of a row.
        "swing" => swing: f64 = 0.0,
        /// Master fader, 0..2.
        "masterVolume" => master_volume: f64 = 0.7,
        /// Meter.
        "timeSignature" => time_signature: TimeSignature,
        /// Metronome on.
        "metronome" => metronome: bool = false,
        /// Metronome level.
        "metronomeVolume" => metronome_volume: f64 = 0.4,
        /// The pattern open in the editor (pattern number).
        "activePattern" => active_pattern: i64 = 0,
        /// Tracks (1..=1024).
        "tracks" => tracks: Vec<Track>,
        /// Track groups.
        "groups" => groups: Vec<Group>,
        /// Instruments (up to 1024).
        "instruments" => instruments: Vec<Instrument>,
        /// Patterns (1..=256).
        "patterns" => patterns: Vec<Pattern>,
        /// The song: pattern numbers in play order (1..=1024). Consecutive equal numbers are the
        /// arranger's repeated blocks. Absent reads as `[0]`.
        "sequence" => sequence: Vec<i64>,
        /// The song (or its loop section) repeats.
        "loopSong" => loop_song: bool = false,
        /// Loop section, honoured while `loopSong` is on.
        "songLoop" => song_loop: SongLoop,
        /// The shared delay.
        "delay" => delay: Delay,
        /// The shared reverb.
        "reverb" => reverb: Reverb,
        /// Mixer routing: buses, outputs, sends and sidechains. Required.
        "routing" => routing: Routing,
        /// Plug-ins and built-in effects, by owner and slot. Authoritative for what is loaded.
        "audioPlugins" => audio_plugins: Vec<PluginDescriptor>,
        /// Audio clips on the song timeline (up to 256).
        "audioClips" => audio_clips: Vec<AudioClip>,
        /// The user's parameter pages, by plug-in `instanceID`.
        "pluginPages" => plugin_pages: BTreeMap<String, PluginPages>,
        /// View preferences.
        "viewState" => view_state: ViewState,
        /// What a MIDI import kept about its source.
        "midiImport" => midi_import: MidiImportInfo,
        /// Content fingerprints of media files.
        "mediaManifest" => media_manifest: Vec<MediaManifestEntry>,
        /// Relink operations applied to this project.
        "mediaRelinkHistory" => media_relink_history: Vec<RelinkRecord>,
    }
}

/// `value` with the keys of every object in sorted order, whatever order `serde_json`'s map keeps
/// (it sorts by default; another crate of the workspace may switch it to insertion order).
fn sorted(value: Value) -> Value {
    static ALREADY_SORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let already_sorted = *ALREADY_SORTED.get_or_init(|| {
        let mut probe = serde_json::Map::new();
        probe.insert("b".to_string(), Value::Null);
        probe.insert("a".to_string(), Value::Null);
        probe.keys().next().map(String::as_str) == Some("a")
    });
    if already_sorted {
        return value;
    }
    sort_keys(value)
}

fn sort_keys(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(sort_keys).collect()),
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(entries.into_iter().map(|(key, value)| (key, sort_keys(value))).collect())
        }
        other => other,
    }
}

impl Project {
    /// Reads a project from JSON text.
    ///
    /// Reading is lenient (see the module documentation): the only failures are text that is not
    /// JSON and JSON that is not an object. Call [`crate::validate::validate`] before using the
    /// result.
    pub fn from_json(text: &str) -> Result<Project> {
        let value: Value = serde_json::from_str(text).map_err(|error| Error::invalid(format!("The project is not valid JSON ({error}).")))?;
        Project::from_value(value)
    }

    /// Reads a project from a JSON value (a bridge payload).
    pub fn from_value(value: Value) -> Result<Project> {
        <Project as Json>::from_json(value).map_err(|_| Error::invalid("The file is not a SuperTracker project."))
    }

    /// The project as a JSON value.
    pub fn to_value(&self) -> Value {
        <Project as Json>::to_json(self)
    }

    /// The project as compact JSON text (for the bridge). Object keys are sorted.
    pub fn to_json(&self) -> String {
        sorted(self.to_value()).to_string()
    }

    /// The project as indented JSON text with sorted keys (the form of project files).
    pub fn to_json_pretty(&self) -> String {
        let value = sorted(self.to_value());
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
    }

    /// The tracks, empty when absent.
    pub fn track_list(&self) -> &[Track] {
        self.tracks.as_deref().unwrap_or(&[])
    }

    /// The instruments, empty when absent.
    pub fn instrument_list(&self) -> &[Instrument] {
        self.instruments.as_deref().unwrap_or(&[])
    }

    /// The patterns, empty when absent.
    pub fn pattern_list(&self) -> &[Pattern] {
        self.patterns.as_deref().unwrap_or(&[])
    }

    /// The song sequence in effect: `sequence`, or `[0]` when absent.
    pub fn song_sequence(&self) -> Vec<i64> {
        self.sequence.clone().unwrap_or_else(|| vec![0])
    }

    /// The pattern with this number (see [`Pattern::number`]).
    pub fn pattern_numbered(&self, number: i64) -> Option<&Pattern> {
        self.pattern_list().iter().enumerate().find(|(position, pattern)| pattern.number(*position) == number).map(|(_, pattern)| pattern)
    }

    /// The plug-in descriptors, empty when absent.
    pub fn plugin_list(&self) -> &[PluginDescriptor] {
        self.audio_plugins.as_deref().unwrap_or(&[])
    }

    /// The audio clips, empty when absent.
    pub fn clip_list(&self) -> &[AudioClip] {
        self.audio_clips.as_deref().unwrap_or(&[])
    }

    /// The session name, `Untitled session` when absent or empty (as the interface names it).
    pub fn display_name(&self) -> &str {
        self.name.as_deref().filter(|name| !name.is_empty()).unwrap_or("Untitled session")
    }

    /// The meter in effect: `(numerator, denominator)`, 4/4 when absent.
    pub fn meter(&self) -> (i64, i64) {
        match &self.time_signature {
            Some(signature) => (signature.numerator(), signature.denominator()),
            None => (4, 4),
        }
    }
}

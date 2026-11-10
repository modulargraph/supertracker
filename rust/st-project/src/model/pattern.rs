//! Patterns: steps (note columns and step effects) and automation lanes.

use crate::json::{model, string_enum};

/// `step.note` of a note-off.
pub const NOTE_OFF: i64 = -1;
/// `step.note` of a cut (immediate silence).
pub const NOTE_CUT: i64 = -2;
/// The value a step without a `note` reads as: no note (an effect-only step).
pub const NOTE_NONE: i64 = -3;
/// A step's gate when it has no `gate` key: such steps predate gate-less notes
/// (`LEGACY_GATE` in `app-project-model.js`, `AudioProject.step`).
pub const LEGACY_GATE: f64 = 0.8;
/// The longest gate, in rows.
pub const MAX_GATE: f64 = 256.0;
/// Note columns a track can have (`MAX_NOTE_COLUMNS` in `app-note-columns.js`, `ST_MAX_NOTE_COLUMNS`).
pub const MAX_NOTE_COLUMNS: i64 = 8;
/// Notes in a chord the engine plays.
pub const MAX_CHORD_NOTES: usize = 8;

string_enum! {
    /// A step effect. Values use the unit of the parameter they set; `Plugin` is a parameter lock
    /// on a plug-in of the step's track.
    pub enum StepEffectKind {
        /// Track volume for the step.
        Volume = "volume",
        /// Track pan.
        Pan = "pan",
        /// Pitch offset in semitones.
        Pitch = "pitch",
        /// Filter cutoff, Hz.
        Cutoff = "cutoff",
        /// Filter resonance.
        Resonance = "resonance",
        /// Delay send.
        Delay = "delay",
        /// Reverb send.
        Reverb = "reverb",
        /// Chance the note plays, 0..1.
        Probability = "probability",
        /// Repeats within the row, 1..8.
        Ratchet = "ratchet",
        /// Reverse playback, 0 or 1.
        Reverse = "reverse",
        /// Sample start offset, 0..1.
        Offset = "offset",
        /// Glide from the previous note, semitones.
        Glide = "glide",
        /// Delay of the onset, 0..0.95 rows.
        Microtiming = "microtiming",
        /// Arpeggio pattern.
        Arpeggio = "arpeggio",
        /// Slice number.
        Slice = "slice",
        /// Wavetable or grain position, 0..1.
        Position = "position",
        /// Plug-in parameter lock: `instanceID`, `address` and a normalized `value`.
        Plugin = "plugin",
    }
}

model! {
    /// One of a step's two effects (`fx1`, `fx2`).
    pub struct StepEffect {
        /// The effect. Absent or unknown: no effect.
        "type" => kind: StepEffectKind,
        /// The effect's value. Plug-in locks are normalized 0..1.
        "value" => value: f64 = 0.0,
        /// Plug-in lock: the `instanceID` of an `audioPlugins` entry on the step's track (for an
        /// instrument effect, the instrument's descriptor).
        "instanceID" => instance_id: String,
        /// Plug-in lock: the parameter address as an exact unsigned 64-bit decimal string
        /// (JSON numbers lose precision above 2^53).
        "address" => address: String,
        /// Plug-in lock: the parameter's display name.
        "name" => name: String,
    }
}

impl StepEffect {
    /// The parameter address of a plug-in lock, when it is an exact `u64` decimal string
    /// (no sign, no padding), as the host requires.
    pub fn plugin_address(&self) -> Option<u64> {
        let text = self.address.as_deref()?;
        let address: u64 = text.parse().ok()?;
        (address.to_string() == text).then_some(address)
    }
}

model! {
    /// A cell of an extra note column (`step.columns[i]` is note column `i + 1`).
    /// Effects belong to column 0, the step itself.
    pub struct ColumnCell {
        /// MIDI note 0..127, [`NOTE_OFF`] or [`NOTE_CUT`].
        "note" => note: i64 = NOTE_NONE,
        /// Instrument index; negative or absent: the track's instrument.
        "instrument" => instrument: i64 = -1,
        /// Velocity 0..127.
        "velocity" => velocity: i64 = 100,
        /// Length in rows; 0 or absent: no gate (columns are newer than gate-less notes).
        "gate" => gate: f64 = 0.0,
    }
}

model! {
    /// One row of one track in a pattern. An empty row is `null` in the file (`None` here).
    pub struct Step {
        /// MIDI note 0..127, [`NOTE_OFF`], [`NOTE_CUT`], or absent for an effect-only step.
        "note" => note: i64 = NOTE_NONE,
        /// Instrument index; negative or absent: the track's instrument.
        "instrument" => instrument: i64 = -1,
        /// Velocity 0..127.
        "velocity" => velocity: i64 = 100,
        /// Length in rows. 0: no gate (the note sounds until the next note, an OFF/CUT or the end
        /// of a one-shot). Absent: [`LEGACY_GATE`].
        "gate" => gate: f64 = LEGACY_GATE,
        /// Semitone offsets of the chord's notes from `note` (up to eight).
        "chord" => chord: Vec<i64>,
        /// First effect.
        "fx1" => fx1: StepEffect,
        /// Second effect.
        "fx2" => fx2: StepEffect,
        /// Extra note columns, up to the track's `noteColumns - 1`; `null` for an empty cell.
        "columns" => columns: Vec<Option<ColumnCell>>,
    }
}

impl Step {
    /// The step's effects that name a known effect, `fx1` first.
    pub fn effects(&self) -> impl Iterator<Item = &StepEffect> {
        [self.fx1.as_ref(), self.fx2.as_ref()].into_iter().flatten().filter(|effect| effect.kind.is_some())
    }
}

string_enum! {
    /// How an automation lane moves between its points.
    pub enum Interpolation {
        /// Straight lines between points.
        Linear = "linear",
        /// Each point holds until the next.
        Step = "step",
    }
}

model! {
    /// One automation point.
    pub struct AutomationPoint {
        /// Position in rows from the pattern's start (fractions allowed), 0..=length.
        "row" => row: f64 = 0.0,
        /// The value, in the target's unit (plug-in parameters: normalized 0..1).
        "value" => value: f64 = 0.0,
    }
}

/// What an automation lane controls, parsed from its `target` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationTarget {
    /// A track or instrument parameter by name (`volume`, `pan`, `cutoff`, ...);
    /// see [`crate::validate::native_automation_range`].
    Native(String),
    /// `send:<routeID>`: the gain of a send or sidechain.
    Send(String),
    /// `plugin:<instrument|effect>:<slot>:<address>`: a plug-in parameter.
    Plugin {
        /// True for the track's instrument plug-in, false for an insert effect.
        instrument: bool,
        /// Insert slot (0 for an instrument).
        slot: i64,
        /// Parameter address.
        address: u64,
    },
}

impl AutomationTarget {
    /// Parses a lane's `target`. `None` for a malformed `send:` or `plugin:` target.
    pub fn parse(target: &str) -> Option<Self> {
        let parts: Vec<&str> = target.split(':').collect();
        match parts.as_slice() {
            ["send", route] if !route.is_empty() => Some(AutomationTarget::Send(route.to_string())),
            ["plugin", kind, slot, address] if *kind == "instrument" || *kind == "effect" => {
                let slot: i64 = slot.parse().ok().filter(|slot| *slot >= 0)?;
                let address: u64 = address.parse().ok()?;
                Some(AutomationTarget::Plugin { instrument: *kind == "instrument", slot, address })
            }
            [name] if !name.is_empty() => Some(AutomationTarget::Native(name.to_string())),
            _ => None,
        }
    }
}

model! {
    /// An automation lane of a pattern.
    ///
    /// A lane belongs either to a track by index (`track`, the engine's own lanes) or to a mixer
    /// channel by identity (`owner`: `track:<routingID>`, `bus:<id>`, `master` or
    /// `instrument:<index>` for an instrument's insert chain).
    pub struct AutomationLane {
        /// Identity of the lane (unique within the pattern).
        "id" => id: String,
        /// Track index of a track lane. Ignored when `owner` is present.
        "track" => track: i64 = 0,
        /// The mixer channel of an owner lane.
        "owner" => owner: String,
        /// What the lane controls; see [`AutomationTarget`]. Absent reads as `volume`.
        "target" => target: String,
        /// Display name.
        "name" => name: String,
        /// How values move between points.
        "interpolation" => interpolation: Interpolation = Interpolation::Linear,
        /// The points (up to 4096).
        "points" => points: Vec<AutomationPoint>,
    }
}

impl AutomationLane {
    /// The lane's target string, `volume` when absent.
    pub fn target_name(&self) -> &str {
        self.target.as_deref().unwrap_or("volume")
    }
}

model! {
    /// A pattern: `steps[track][row]` and its automation lanes.
    pub struct Pattern {
        /// The number the song sequence refers to, 0..255, unique. Absent: the pattern's position.
        "id" => id: i64,
        /// Display name.
        "name" => name: String,
        /// Rows, 1..256.
        "length" => length: i64 = 64,
        /// One array of rows per track; `null` for an empty row.
        "steps" => steps: Vec<Vec<Option<Step>>>,
        /// Automation lanes.
        "automation" => automation: Vec<AutomationLane>,
        /// Block colour in the song view.
        "color" => color: String,
    }
}

impl Pattern {
    /// The pattern's number: its `id`, or `position` (its index in `project.patterns`) when absent.
    pub fn number(&self, position: usize) -> i64 {
        self.id.unwrap_or(position as i64)
    }

    /// The step at `track`, `row`, if there is one.
    pub fn step(&self, track: usize, row: usize) -> Option<&Step> {
        self.steps.as_ref()?.get(track)?.get(row)?.as_ref()
    }

    /// An empty pattern for `tracks` tracks, named as the interface names new patterns.
    pub fn empty(id: i64, length: usize, tracks: usize) -> Pattern {
        Pattern {
            id: Some(id),
            name: Some(format!("Pattern {:02}", id + 1)),
            length: Some(length as i64),
            steps: Some(vec![vec![None; length]; tracks]),
            automation: Some(Vec::new()),
            ..Pattern::default()
        }
    }
}

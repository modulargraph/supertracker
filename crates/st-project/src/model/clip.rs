//! Audio clips on the song timeline, their takes, and Track Freeze records.

use crate::json::model;

/// Takes a clip can keep (`maxTakes` in `clip-takes-project.js`).
pub const MAX_TAKES: usize = 32;

model! {
    /// One alternative recording of a clip.
    pub struct Take {
        /// Identity within the clip.
        "id" => id: String,
        /// Display name.
        "name" => name: String,
        /// The audio file (media path).
        "path" => path: String,
        /// Where in the file the clip starts, seconds.
        "sourceOffset" => source_offset: f64 = 0.0,
        /// Length of the file in seconds, when known.
        "sourceDuration" => source_duration: f64,
    }
}

model! {
    /// A clip's takes. The active take's `path` and `sourceOffset` are mirrored on the clip itself,
    /// which is what playback reads.
    pub struct TakeSet {
        /// The `id` of the take the clip plays.
        "activeID" => active_id: String,
        /// The takes (1..=32).
        "takes" => takes: Vec<Take>,
    }
}

impl TakeSet {
    /// Index of the active take, when exactly one take has `activeID`.
    pub fn active_index(&self) -> Option<usize> {
        let active = self.active_id.as_deref()?;
        let takes = self.takes.as_ref()?;
        if takes.len() > MAX_TAKES {
            return None;
        }
        let mut matches = takes.iter().enumerate().filter(|(_, take)| take.id.as_deref() == Some(active));
        let first = matches.next()?;
        matches.next().is_none().then_some(first.0)
    }
}

model! {
    /// An audio clip on the song timeline (`project.audioClips`).
    pub struct AudioClip {
        /// Identity.
        "id" => id: String,
        /// Display name.
        "name" => name: String,
        /// Track index the clip plays on.
        "track" => track: i64 = 0,
        /// The audio file (media path).
        "path" => path: String,
        /// Start on the timeline, in quarter-note beats.
        "startBeat" => start_beat: f64 = 0.0,
        /// Length on the timeline, in beats (> 0).
        "durationBeats" => duration_beats: f64 = 0.0,
        /// Where in the file the clip starts, seconds.
        "sourceOffset" => source_offset: f64 = 0.0,
        /// Length of the file in seconds, when known.
        "sourceDuration" => source_duration: f64,
        /// Gain, 1 = unchanged.
        "gain" => gain: f64 = 1.0,
        /// Fade-in, seconds.
        "fadeIn" => fade_in: f64 = 0.005,
        /// Fade-out, seconds.
        "fadeOut" => fade_out: f64 = 0.005,
        /// Muted.
        "muted" => muted: bool = false,
        /// Alternative takes.
        "takeSet" => take_set: TakeSet,
    }
}

impl AudioClip {
    /// The clip's audio file, when it names one.
    pub fn file(&self) -> Option<&str> {
        self.path.as_deref().filter(|path| !path.is_empty())
    }
}

model! {
    /// The looping pass of a pattern print.
    pub struct PrintLoop {
        /// The audio file (media path).
        "path" => path: String,
        /// SHA-256 of the file as printed.
        "sha256" => sha256: String,
        /// Frames in the file.
        "frames" => frames: i64,
    }
}

model! {
    /// A frozen track's print of one pattern: the first pass with its tail, and the steady-state
    /// looping pass.
    pub struct PatternPrint {
        /// Pattern number, 0..255.
        "pattern" => pattern: i64,
        /// First pass + tail (media path).
        "path" => path: String,
        /// SHA-256 of that file as printed.
        "sha256" => sha256: String,
        /// Frames in that file.
        "frames" => frames: i64,
        /// Frames of one pass of the pattern.
        "passFrames" => pass_frames: i64,
        /// Passes rendered to reach the steady state.
        "passes" => passes: i64 = 2,
        /// The looping pass.
        "loop" => loop_print: PrintLoop,
    }
}

model! {
    /// Track Freeze (`tracks[i].frozen`): the track's instrument and inserts printed to audio.
    /// Written by the host's `freeze.track`; the interface stores it and removes it on Unfreeze.
    ///
    /// The print's `sha256` is its identity: a file with other content never replaces it
    /// (saving and relinking both check).
    pub struct FreezeRecord {
        /// Identity of this freeze.
        "id" => id: String,
        /// The song print (media path).
        "path" => path: String,
        /// SHA-256 of the song print as printed.
        "sha256" => sha256: String,
        /// Sample rate of the prints, 8000..384000.
        "sampleRate" => sample_rate: f64,
        /// Frames in the song print (`musicalFrames + tailFrames`).
        "frames" => frames: i64,
        /// Frames up to the song's musical end.
        "musicalFrames" => musical_frames: i64,
        /// Frames of tail after it.
        "tailFrames" => tail_frames: i64,
        /// The song's end in beats when printed.
        "musicalEndBeat" => musical_end_beat: f64,
        /// The tail asked for, seconds.
        "tailSeconds" => tail_seconds: f64 = 0.0,
        /// Plug-in instances unloaded while the track is frozen.
        "dormantPluginIDs" => dormant_plugin_ids: Vec<String>,
        /// The host's fingerprint of what was printed, to detect a stale print. Opaque here.
        "fingerprint" => fingerprint: String,
        /// When the print was made (ISO 8601).
        "createdAt" => created_at: String,
        /// Prints of the patterns the track plays, for Pattern mode.
        "patternPrints" => pattern_prints: Vec<PatternPrint>,
        /// Patterns with content that were not printed.
        "unprintedPatterns" => unprinted_patterns: Vec<i64>,
        /// `auto` (tail ends at silence) or `fixed`.
        "tailMode" => tail_mode: String,
        /// `offline` (rendered) or `recorded` (hardware return captured in real time).
        "method" => method: String,
    }
}

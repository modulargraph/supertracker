//! The immutable project snapshot: `Sample`, `Zone`, `Instrument`, `Step`, `Lane`, `Column`,
//! `Pattern`, the clip structures and `Project`, plus the project-only arithmetic the control
//! and render threads share (`pattern_rows`, `beat_for_row`, song duration).
//!
//! A project is edited as a draft on the control thread, deep-copied at commit (pattern columns
//! and lane lists are shared copy-on-write) and handed to the render thread, which only reads it.

use super::abi::*;
use super::tables::ChannelTables;
use super::util::{cmax, cmin};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

/// Decoded PCM. Owned by `Control::samples`; freed by `collect()` once `refs` is zero.
pub struct Sample {
    pub refs: AtomicI32,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub rate: f64,
}

/// A counted reference to a `Sample` held by project data (C++ `Zone`/`Instrument` copy
/// constructors and destructors). Cloning and dropping only touch the atomic count, never free:
/// the sample itself is destroyed by `collect()` on the control thread.
pub struct SampleRef(NonNull<Sample>);

// SAFETY: a SampleRef is a pointer to a heap `Sample` whose PCM is immutable after publication
// and whose count is atomic; the sample outlives every reference because `collect()` frees only
// samples whose count is zero.
unsafe impl Send for SampleRef {}
unsafe impl Sync for SampleRef {}

impl SampleRef {
    /// Takes a new counted reference.
    ///
    /// # Safety
    /// `sample` must point to a live `Sample` owned by the engine's sample list.
    pub unsafe fn retain(sample: *const Sample) -> Option<SampleRef> {
        let pointer = NonNull::new(sample as *mut Sample)?;
        // SAFETY: the caller guarantees the sample is alive.
        unsafe { pointer.as_ref() }.refs.fetch_add(1, Ordering::Relaxed);
        Some(SampleRef(pointer))
    }
    #[inline(always)]
    pub fn as_ptr(&self) -> *const Sample {
        self.0.as_ptr()
    }
    #[inline(always)]
    pub fn get(&self) -> &Sample {
        // SAFETY: this reference holds a count, so `collect()` has not freed the sample.
        unsafe { self.0.as_ref() }
    }
}

impl Clone for SampleRef {
    fn clone(&self) -> Self {
        self.get().refs.fetch_add(1, Ordering::Relaxed);
        SampleRef(self.0)
    }
}

impl Drop for SampleRef {
    fn drop(&mut self) {
        self.get().refs.fetch_sub(1, Ordering::Release);
    }
}

/// The raw sample pointer of an optional reference (null when absent), as the render thread
/// stores it in voices.
#[inline(always)]
pub fn sample_ptr(sample: &Option<SampleRef>) -> *const Sample {
    match sample {
        Some(reference) => reference.as_ptr(),
        None => std::ptr::null(),
    }
}

#[derive(Clone)]
pub struct Zone {
    pub sample: Option<SampleRef>,
    pub key_low: i32,
    pub key_high: i32,
    pub velocity_low: i32,
    pub velocity_high: i32,
    pub root_note: i32,
    pub tune: f32,
    pub volume: f32,
}

impl Default for Zone {
    fn default() -> Self {
        Zone {
            sample: None,
            key_low: 0,
            key_high: 127,
            velocity_low: 0,
            velocity_high: 127,
            root_note: 60,
            tune: 0.0,
            volume: 1.0,
        }
    }
}

#[derive(Clone)]
pub struct Instrument {
    pub p: STInstrument,
    pub sample: Option<SampleRef>,
    pub slices: Vec<f32>,
    pub zones: Vec<Zone>,
    /// Wavetable frame size detected in the sample; 0 unknown.
    pub cycle_frames: i32,
}

impl Default for Instrument {
    fn default() -> Self {
        Instrument { p: default_instrument(), sample: None, slices: Vec::new(), zones: Vec::new(), cycle_frames: 0 }
    }
}

impl Instrument {
    pub fn set_sample(&mut self, sample: Option<SampleRef>) {
        self.sample = sample;
        self.cycle_frames = 0;
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct PluginStepFX {
    pub present: bool,
    /// The host destination whose parameter the lock sets.
    pub destination: i32,
    pub address: u64,
    pub value: f64,
}

impl PluginStepFX {
    pub const NONE: PluginStepFX = PluginStepFX { present: false, destination: -1, address: 0, value: 0.0 };
}

impl Default for PluginStepFX {
    fn default() -> Self {
        PluginStepFX::NONE
    }
}

#[derive(Clone, Copy)]
pub struct Step {
    pub plugin_fx: [PluginStepFX; 2],
    pub present: bool,
    pub p: STStep,
    pub chord: [i32; ST_MAX_CHORD],
    pub count: i32,
}

impl Step {
    pub const BLANK: Step = Step {
        plugin_fx: [PluginStepFX::NONE; 2],
        present: false,
        p: default_step(),
        chord: [0; ST_MAX_CHORD],
        count: 1,
    };
}

impl Default for Step {
    fn default() -> Self {
        Step::BLANK
    }
}

static BLANK_STEP: Step = Step::BLANK;

#[derive(Clone, Copy)]
pub struct Point {
    pub row: f64,
    pub value: f64,
}

#[derive(Clone)]
/// An automation lane. A native lane drives `target` (`ST_TARGET_*`) of `track`; a host lane
/// drives the parameter at `address` of the host destination `target` (its `track` is -1).
pub struct Lane {
    pub track: i32,
    pub target: i32,
    pub interpolation: i32,
    pub address: u64,
    pub points: Vec<Point>,
}

/// Copy-on-write pattern storage. Each (pattern,track) column and each automation lane list is
/// shared between the draft and every published Project until it is edited, so a commit copies
/// pointers, not steps. Shared pointers are only copied or released on the control thread
/// (drafts, commits and collect()); the render thread only reads through the published Project,
/// so a strong count of 1 means "owned by this draft alone".
#[derive(Clone, Default)]
pub struct Column {
    pub rows: Vec<Step>,
}

/// Note columns: column 0 of a track is `steps[track]`; extra note columns
/// 1..ST_MAX_NOTE_COLUMNS-1 live in `extra[track*(ST_MAX_NOTE_COLUMNS-1)+column-1]`,
/// copy-on-write per (pattern,track,column) like column 0. Each column is its own sequencer lane
/// on the track's channel: lane id track+column*ST_TRACK_SANITY_LIMIT.
pub const EXTRA_COLUMNS: i32 = ST_MAX_NOTE_COLUMNS - 1;

/// The track of a sequencer lane id.
#[inline(always)]
pub fn lane_track(lane: i32) -> i32 {
    if lane < 0 {
        lane
    } else {
        lane % ST_TRACK_SANITY_LIMIT
    }
}

#[derive(Clone)]
pub struct Pattern {
    pub length: i32,
    pub steps: Vec<Option<Arc<Column>>>,
    pub extra: Vec<Option<Arc<Column>>>,
    pub lanes: Option<Arc<Vec<Lane>>>,
    pub host_lanes: Option<Arc<Vec<Lane>>>,
    pub lanes_dirty: bool,
}

impl Default for Pattern {
    fn default() -> Self {
        Pattern { length: 64, steps: Vec::new(), extra: Vec::new(), lanes: None, host_lanes: None, lanes_dirty: false }
    }
}

impl Pattern {
    fn at(list: &[Option<Arc<Column>>], index: usize, row: i32) -> &Step {
        match list.get(index) {
            Some(Some(column)) if row >= 0 => column.rows.get(row as usize).unwrap_or(&BLANK_STEP),
            _ => &BLANK_STEP,
        }
    }
    #[inline]
    pub fn step(&self, lane: i32, row: i32) -> &Step {
        if lane < 0 {
            return &BLANK_STEP;
        }
        Self::at(&self.steps, lane as usize, row)
    }
    /// column 1..ST_MAX_NOTE_COLUMNS-1; any other column reads as empty.
    #[inline]
    pub fn column_step(&self, track: i32, column: i32, row: i32) -> &Step {
        if track < 0 || column < 1 || column > EXTRA_COLUMNS {
            return &BLANK_STEP;
        }
        Self::at(&self.extra, track as usize * EXTRA_COLUMNS as usize + (column - 1) as usize, row)
    }
    pub fn has_extra_columns(&self, track: i32) -> bool {
        if track < 0 {
            return false;
        }
        let first = track as usize * EXTRA_COLUMNS as usize;
        if first >= self.extra.len() {
            return false;
        }
        let end = cmin(first + EXTRA_COLUMNS as usize, self.extra.len());
        self.extra[first..end].iter().any(|column| column.is_some())
    }
    pub fn has_column(&self, lane: i32) -> bool {
        lane >= 0 && matches!(self.steps.get(lane as usize), Some(Some(_)))
    }
    #[inline]
    pub fn lane_list(&self) -> &[Lane] {
        match &self.lanes {
            Some(lanes) => lanes,
            None => &[],
        }
    }
    #[inline]
    pub fn host_lane_list(&self) -> &[Lane] {
        match &self.host_lanes {
            Some(lanes) => lanes,
            None => &[],
        }
    }
    /// The column at `index`, made unique to this draft (created, or copied when shared).
    fn own(list: &mut Vec<Option<Arc<Column>>>, index: usize) -> &mut Column {
        if index >= list.len() {
            list.resize(index + 1, None);
        }
        Arc::make_mut(list[index].get_or_insert_with(|| Arc::new(Column::default())))
    }
    fn row_of(length: i32, column: &mut Column, row: usize) -> &mut Step {
        if column.rows.len() <= row {
            column.rows.resize(cmax(length, 0) as usize, Step::BLANK);
        }
        // Callers check row < length, so the row exists after the resize above.
        &mut column.rows[row]
    }
    /// Callers check `0 <= row < length` and `lane >= 0`.
    pub fn mutable_step(&mut self, lane: i32, row: i32) -> &mut Step {
        let length = self.length;
        Self::row_of(length, Self::own(&mut self.steps, lane as usize), row as usize)
    }
    /// Callers check `0 <= row < length`, `track >= 0` and `1 <= column <= EXTRA_COLUMNS`.
    pub fn mutable_column_step(&mut self, track: i32, column: i32, row: i32) -> &mut Step {
        let length = self.length;
        let index = track as usize * EXTRA_COLUMNS as usize + (column - 1) as usize;
        Self::row_of(length, Self::own(&mut self.extra, index), row as usize)
    }
    pub fn mutable_lanes(&mut self, host: bool) -> &mut Vec<Lane> {
        self.lanes_dirty = true;
        let list = if host { &mut self.host_lanes } else { &mut self.lanes };
        Arc::make_mut(list.get_or_insert_with(|| Arc::new(Vec::new())))
    }
    pub fn resize(&mut self, n: i32) {
        if n < self.length {
            let rows = n as usize;
            for list in [&mut self.steps, &mut self.extra] {
                for index in 0..list.len() {
                    if matches!(&list[index], Some(column) if column.rows.len() > rows) {
                        Self::own(list, index).rows.truncate(rows);
                    }
                }
            }
        }
        self.length = n;
    }
}

#[derive(Clone, Copy)]
pub struct AudioClip {
    pub asset: i32,
    pub track: i32,
    pub start_beat: f64,
    pub duration_beats: f64,
    pub source_offset: f64,
    pub gain: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub muted: bool,
}

/// Pattern-mode clip (Track Freeze print of one pattern); see `add_pattern_clip`.
#[derive(Clone, Copy)]
pub struct PatternClip {
    pub head_asset: i32,
    pub loop_asset: i32,
    pub track: i32,
    pub pattern: i32,
    pub gain: f32,
}

/// Song-loop tail (Track Freeze print beyond its musical end); see `add_song_tail_clip`.
#[derive(Clone, Copy)]
pub struct SongTailClip {
    pub asset: i32,
    pub track: i32,
    pub start: i64,
    pub gain: f32,
}

/// Per-track voice settings (`set_track_voice`).
#[derive(Clone, Copy)]
pub struct TrackVoice {
    pub choke: i32,
    pub scope: i32,
    pub mode: i32,
}

impl Default for TrackVoice {
    fn default() -> Self {
        TrackVoice { choke: 0, scope: 0, mode: -1 }
    }
}

#[derive(Clone)]
pub struct Project {
    pub bpm: f64,
    pub lpb: i32,
    pub track_count: i32,
    pub active_pattern: i32,
    pub swing: f32,
    /// Song section loop: inclusive sequence indices, active only while loopSong is on; -1 disables.
    pub song_loop_start: i32,
    pub song_loop_end: i32,
    pub loop_song: bool,
    pub metronome: bool,
    pub numerator: i32,
    pub denominator: i32,
    pub metronome_volume: f32,
    /// Tracks beyond the vector use the defaults.
    pub track_voices: Vec<TrackVoice>,
    /// The mixer values of the tracks: the host mixes, the sequencer plays their lanes and step
    /// effects and reports every value as an event. Holds at least return_bus() entries after
    /// commit; channels below the return bus but at or above track_count are disabled.
    pub tracks: Vec<STTrack>,
    /// Whether a track is soloed (cached at commit): with `STTrack::mute`, what gates the notes
    /// of hardware MIDI instruments.
    pub any_solo: bool,
    pub instruments: Vec<Instrument>,
    /// 256 timeline audio assets.
    pub audio_assets: Vec<Zone>,
    pub audio_clips: Vec<AudioClip>,
    pub pattern_clips: Vec<PatternClip>,
    pub song_tail_clips: Vec<SongTailClip>,
    pub patterns: Vec<Pattern>,
    pub sequence: Vec<i32>,
    /// Intrusive link of the render thread's retired-project list.
    pub retired_next: *mut Project,
    /// Render-owned per-channel state, sized for this project at commit.
    pub tables: Option<Arc<ChannelTables>>,
    /// Identifies the published pattern content: 0 in a draft whose patterns were edited; commit
    /// then assigns a process-unique value. Unedited commits keep it.
    pub pattern_epoch: u64,
    /// Host destination numbering of host lanes and host locks (`set_host_automation_map`).
    pub host_map: u64,
}

pub const AUDIO_ASSETS: usize = 256;

impl Default for Project {
    fn default() -> Self {
        Project::new()
    }
}

impl Project {
    pub fn new() -> Project {
        Project {
            bpm: 120.0,
            lpb: 4,
            track_count: 8,
            active_pattern: 0,
            swing: 0.0,
            song_loop_start: -1,
            song_loop_end: -1,
            loop_song: false,
            metronome: false,
            numerator: 4,
            denominator: 4,
            metronome_volume: 0.3,
            track_voices: Vec::new(),
            tracks: vec![default_track(); ST_MIN_RETURN_BUS as usize],
            any_solo: false,
            instruments: vec![Instrument::default(); 64],
            audio_assets: vec![Zone::default(); AUDIO_ASSETS],
            audio_clips: Vec::new(),
            pattern_clips: Vec::new(),
            song_tail_clips: Vec::new(),
            patterns: vec![Pattern::default()],
            sequence: vec![0],
            retired_next: std::ptr::null_mut(),
            tables: None,
            pattern_epoch: 0,
            host_map: 0,
        }
    }

    /// Valid index into instruments (render thread: a stale step or mix may name a removed one).
    #[inline(always)]
    pub fn has_instrument(&self, index: i32) -> bool {
        index >= 0 && (index as usize) < self.instruments.len()
    }
    #[inline(always)]
    pub fn return_bus(&self) -> i32 {
        return_bus(self.track_count)
    }
    #[inline(always)]
    pub fn channels(&self) -> i32 {
        channel_count(self.track_count)
    }

    /// Length of pattern `index`. The sequence and active pattern are clamped when they are set
    /// and patterns never shrink within a project, so the index is valid; an invalid one reads
    /// as the last pattern instead of faulting.
    #[inline]
    pub fn pattern_length(&self, index: i32) -> i32 {
        let last = self.patterns.len().saturating_sub(1);
        let index = cmin(cmax(index, 0) as usize, last);
        self.patterns.get(index).map_or(64, |pattern| pattern.length)
    }

    /// The pattern at `index` (see `pattern_length` for out-of-range indices).
    #[inline]
    pub fn pattern(&self, index: i32) -> Option<&Pattern> {
        let last = self.patterns.len().saturating_sub(1);
        self.patterns.get(cmin(cmax(index, 0) as usize, last))
    }

    /// The sample of timeline audio asset `asset`, if any.
    #[inline]
    pub fn asset_sample(&self, asset: i32) -> Option<&Sample> {
        self.audio_assets.get(asset as u32 as usize)?.sample.as_ref().map(|sample| sample.get())
    }

    /// `patternRows`: the rows of a pattern in even-row units (an odd length ends on a long,
    /// swung row). The sum is formed in single precision, as in C++ (`int + float`).
    #[inline]
    pub fn pattern_rows(&self, index: i32) -> f64 {
        let n = self.pattern_length(index);
        (n as f32 + if n % 2 != 0 { self.swing } else { 0.0 }) as f64
    }

    /// `beatForRow`: pattern-relative quarter beat of row `r`. Single precision, as in C++
    /// (`(int + float) / int`).
    #[inline]
    pub fn beat_for_row(&self, r: i32) -> f64 {
        ((r as f32 + if r % 2 != 0 { self.swing } else { 0.0 }) / self.lpb as f32) as f64
    }

    /// How long a timeline clip plays, in seconds: its duration in beats at the project tempo,
    /// or what its source holds after the offset when that is shorter (or no duration is set).
    pub fn clip_seconds(&self, clip: &AudioClip) -> f64 {
        let Some(sample) = self.asset_sample(clip.asset) else { return 0.0 };
        let available = cmax(0.0, sample.l.len() as f64 / sample.rate - clip.source_offset);
        if clip.duration_beats > 0.0 {
            cmin(available, clip.duration_beats * 60.0 / self.bpm)
        } else {
            available
        }
    }

    /// The song's length in seconds: its sequence, or the end of the last unmuted clip when
    /// that is later.
    pub fn song_duration(&self) -> f64 {
        let mut rows = 0.0f64;
        for &index in &self.sequence {
            rows += self.pattern_rows(index);
        }
        let mut seconds = rows * 60.0 / self.bpm / self.lpb as f64;
        for clip in &self.audio_clips {
            if !clip.muted {
                seconds = cmax(seconds, clip.start_beat * 60.0 / self.bpm + self.clip_seconds(clip));
            }
        }
        seconds
    }

    /// The song's length in frames: the seconds of [`Project::song_duration`] times the rate
    /// (that product, in that order, is what the transport compares against).
    pub fn effective_song_frames(&self, sample_rate: f64) -> f64 {
        self.song_duration() * sample_rate
    }
}

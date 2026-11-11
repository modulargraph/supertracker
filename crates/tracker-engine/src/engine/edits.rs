//! The control thread's operations as methods of [`Control`]: the project setters and the
//! command producers. A host reaches them through `host::ControlHalf::parts`.
//!
//! Every setter sanitizes its input (clamping, finite fallbacks, silently ignoring out-of-range
//! indices) and edits the draft; nothing here touches the render
//! thread except through the published project and the command ring, whose single producer is
//! the control thread (hence `&mut self` on the producers).

use super::abi::*;
use super::control::Control;
use super::project::*;
use super::state::*;
use super::util::{clamp, cmin, finite, finite64};
use std::sync::atomic::{AtomicI32, Ordering};

/// Planar PCM handed to the engine (copied; `right` defaults to `left`).
#[derive(Clone, Copy)]
pub struct Pcm<'a> {
    pub left: &'a [f32],
    pub right: Option<&'a [f32]>,
    pub rate: f64,
}

impl Pcm<'_> {
    /// 1..INT32_MAX frames on every channel at 1000..768000 Hz.
    fn valid(&self) -> bool {
        let frames = self.left.len();
        frames >= 1
            && frames <= i32::MAX as usize
            && self.right.is_none_or(|right| right.len() >= frames)
            && self.rate.is_finite()
            && self.rate >= 1000.0
            && self.rate <= 768000.0
    }

    /// Copy into a new sample, sanitizing every value. `None` when memory is exhausted (a failed
    /// load, not an abort).
    fn to_sample(self) -> Option<Box<Sample>> {
        let frames = self.left.len();
        let (mut l, mut r) = (Vec::new(), Vec::new());
        l.try_reserve_exact(frames).ok()?;
        r.try_reserve_exact(frames).ok()?;
        let right = self.right.unwrap_or(self.left);
        l.extend(self.left.iter().map(|&x| clamp(finite(x, 0.0), -8.0, 8.0)));
        r.extend(right.iter().take(frames).map(|&x| clamp(finite(x, 0.0), -8.0, 8.0)));
        Some(Box::new(Sample { refs: AtomicI32::new(0), l, r, rate: self.rate }))
    }
}

/// The step's note fields as the engine stores them (`set_step`'s sanitizing).
fn sanitize_step_note(step: &mut STStep, instruments: usize) {
    step.instrument = clamp(step.instrument, -1, instruments as i32 - 1);
    step.velocity = clamp(step.velocity, 0, 127);
    step.gate = if finite(step.gate, 0.0) > 0.0 { clamp(step.gate, 0.01, 256.0) } else { 0.0 };
}

/// Up to `ST_MAX_CHORD` chord offsets, clamped to +/-96 semitones; (1, zeros) without a chord.
fn sanitize_chord(chord: Option<&[i32]>) -> (i32, [i32; ST_MAX_CHORD]) {
    let mut notes = [0; ST_MAX_CHORD];
    match chord {
        Some(chord) if !chord.is_empty() => {
            let count = cmin(chord.len(), ST_MAX_CHORD);
            for (slot, &note) in notes.iter_mut().zip(chord) {
                *slot = clamp(note, -96, 96);
            }
            (count as i32, notes)
        }
        _ => (1, notes),
    }
}

/// Shared by native and host lanes: find or create the lane matching `key` (at most 256 lanes
/// per pattern) and append a point (at most 4096 per lane).
fn add_lane_point(target: &mut Pattern, host: bool, key: impl Fn(&Lane) -> bool, fresh: Lane, row: f64, value: f64) {
    {
        let shared = if host { target.host_lane_list() } else { target.lane_list() };
        let mut known = false;
        for lane in shared {
            if key(lane) {
                known = lane.points.len() < 4096;
                if !known {
                    return;
                }
                break;
            }
        }
        if !known && shared.len() >= 256 {
            return;
        }
    }
    let length = target.length as f64;
    let lanes = target.mutable_lanes(host);
    let index = match lanes.iter().position(&key) {
        Some(index) => index,
        None => {
            if lanes.len() >= 256 {
                return;
            }
            lanes.push(fresh);
            lanes.len() - 1
        }
    };
    let points = &mut lanes[index].points;
    if points.len() < 4096 {
        points.push(Point { row: clamp(row, 0.0, length), value });
    }
}

impl Control {
    // ---- update transactions ----

    /// Opens an update: a fresh draft of the published project. Edits made until `commit` are
    /// published together.
    pub fn begin_update(&mut self, sh: &Shared) {
        self.collect(sh);
        self.draft = Some(Box::new((*self.control).clone()));
        self.updating = true;
    }

    /// A fresh draft that keeps instruments and timeline assets.
    pub fn clear_project(&mut self) {
        let mut fresh = Box::new(Project::new());
        fresh.pattern_epoch = 0;
        fresh.instruments = self.control.instruments.clone();
        fresh.audio_assets = self.control.audio_assets.clone();
        self.draft = Some(fresh);
        self.updating = true;
    }

    /// Discards the draft: every edit since the last commit is dropped.
    pub fn cancel_update(&mut self) {
        self.draft = None;
        self.updating = false;
    }

    /// "Publishes at once outside an update": commit unless a begin/commit transaction is open.
    fn publish_outside_update(&mut self, sh: &Shared) -> bool {
        self.updating || self.commit(sh).is_ok()
    }

    /// Give a new sample to the engine's sample list and return a counted reference to it.
    fn adopt_sample(&mut self, sample: Box<Sample>) -> Option<SampleRef> {
        let raw: *const Sample = &*sample;
        self.samples.push(sample);
        // SAFETY: `raw` points into the box just pushed onto `samples`, which keeps it alive (and
        // at the same address) until `collect()` finds its count at zero.
        unsafe { SampleRef::retain(raw) }
    }

    // ---- project settings ----

    /// Tempo (20..999 BPM), lines per beat (1..32) and swing (0..0.95), each clamped; non-finite
    /// values fall back to the defaults.
    pub fn set_timing(&mut self, bpm: f64, lines_per_beat: i32, swing: f32) {
        let p = self.edit();
        p.bpm = clamp(finite64(bpm, 120.0), 20.0, 999.0);
        p.lpb = clamp(lines_per_beat, 1, 32);
        p.swing = clamp(finite(swing, 0.0), 0.0, 0.95);
    }

    /// The metronome: on/off, the meter that places its accents, and its volume.
    pub fn set_metronome(&mut self, enabled: bool, numerator: i32, denominator: i32, volume: f32) {
        let p = self.edit();
        p.metronome = enabled;
        p.numerator = clamp(numerator, 1, 16);
        p.denominator = if matches!(denominator, 2 | 4 | 8 | 16) { denominator } else { 4 };
        p.metronome_volume = clamp(finite(volume, 0.3), 0.0, 1.0);
    }

    /// Number of tracks (1..=`ST_TRACK_SANITY_LIMIT`); false for a count out of range.
    pub fn set_track_count(&mut self, count: i32) -> bool {
        if count < 1 || count > ST_TRACK_SANITY_LIMIT {
            return false;
        }
        let p = self.edit();
        p.track_count = count;
        let base = p.return_bus() as usize;
        if p.tracks.len() < base {
            p.tracks.resize(base, default_track());
        }
        true
    }

    /// Mixer values of one track (clamped) and its instrument; an index outside
    /// 0..`ST_TRACK_SANITY_LIMIT` is ignored. The engine does not apply volume, pan or sends: it
    /// reports them, and what their lanes and step effects make of them, as events. Mute and solo
    /// gate the notes of hardware MIDI instruments.
    pub fn set_track(&mut self, index: i32, mut track: STTrack) {
        if index < 0 || index >= ST_TRACK_SANITY_LIMIT {
            return;
        }
        track.volume = clamp(finite(track.volume, 0.8), 0.0, 2.0);
        track.pan = clamp(finite(track.pan, 0.0), -1.0, 1.0);
        track.delay_send = clamp(finite(track.delay_send, 0.0), 0.0, 1.0);
        track.reverb_send = clamp(finite(track.reverb_send, 0.0), 0.0, 1.0);
        track.instrument = clamp(track.instrument, 0, self.view().instruments.len() as i32 - 1);
        let p = self.edit();
        let index = index as usize;
        if p.tracks.len() <= index {
            p.tracks.resize(index + 1, default_track());
        }
        p.tracks[index] = track;
    }

    /// Per-track voice settings: choke group (0..=127), its scope and the voice mode (-1..=2).
    /// False for values out of range.
    pub fn set_track_voice(&mut self, track: i32, choke_group: i32, choke_scope: i32, voice_mode: i32) -> bool {
        if track < 0
            || track >= ST_TRACK_SANITY_LIMIT
            || choke_group < 0
            || choke_group > 127
            || choke_scope < 0
            || choke_scope > ST_TRACK_SANITY_LIMIT
            || voice_mode < -1
            || voice_mode > 2
        {
            return false;
        }
        let p = self.edit();
        let track = track as usize;
        if p.track_voices.len() <= track {
            p.track_voices.resize(track + 1, TrackVoice::default());
        }
        p.track_voices[track] = TrackVoice { choke: choke_group, scope: choke_scope, mode: voice_mode };
        true
    }

    /// Number of instrument slots (1..=`ST_INSTRUMENT_SANITY_LIMIT`); false for a count out of range.
    pub fn set_instrument_count(&mut self, count: i32) -> bool {
        if count < 1 || count > ST_INSTRUMENT_SANITY_LIMIT {
            return false;
        }
        self.edit().instruments.resize(count as usize, Instrument::default());
        true
    }

    /// Settings of one instrument (clamped to their ranges); an index beyond the instrument count
    /// is ignored.
    pub fn set_instrument(&mut self, index: i32, instrument: STInstrument) {
        if !self.view().has_instrument(index) {
            return;
        }
        let mut p = instrument;
        p.type_ = if p.type_ == ST_INSTRUMENT_AU || p.type_ == ST_INSTRUMENT_MIDI { p.type_ } else { ST_INSTRUMENT_SAMPLE };
        p.wavetable_position = clamp(finite(p.wavetable_position, 0.0), 0.0, 1.0);
        if p.wavetable_frame_size < ST_WAVETABLE_MIN_FRAME || p.wavetable_frame_size > ST_WAVETABLE_MAX_FRAME {
            p.wavetable_frame_size = 0;
        }
        p.voice_mode = clamp(p.voice_mode, 0, 2);
        p.mode = clamp(p.mode, 0, 6);
        p.root_note = clamp(p.root_note, 0, 127);
        p.filter_type = clamp(p.filter_type, 0, 2);
        p.slice_count = clamp(p.slice_count, 1, 128);
        p.bit_depth = clamp(p.bit_depth, 2, 16);
        p.volume = clamp(finite(p.volume, 0.7), 0.0, 2.0);
        p.pan = clamp(finite(p.pan, 0.0), -1.0, 1.0);
        p.tune = clamp(finite(p.tune, 0.0), -96.0, 96.0);
        p.start = clamp(finite(p.start, 0.0), 0.0, 0.999999);
        p.end = clamp(finite(p.end, 1.0), p.start + 0.000001, 1.0);
        p.loop_start = clamp(finite(p.loop_start, 0.0), 0.0, 0.999999);
        p.loop_end = clamp(finite(p.loop_end, 1.0), p.loop_start + 0.000001, 1.0);
        p.attack = clamp(finite(p.attack, 0.005), 0.0, 60.0);
        p.decay = clamp(finite(p.decay, 0.2), 0.0, 60.0);
        p.sustain = clamp(finite(p.sustain, 0.7), 0.0, 1.0);
        p.release = clamp(finite(p.release, 0.2), 0.0001, 60.0);
        p.cutoff = clamp(finite(p.cutoff, 18000.0), 20.0, 22000.0);
        p.resonance = clamp(finite(p.resonance, 0.0), 0.0, 0.99);
        p.drive = clamp(finite(p.drive, 0.0), 0.0, 1.0);
        p.lfo_rate = clamp(finite(p.lfo_rate, 0.0), 0.0, 100.0);
        p.lfo_depth = clamp(finite(p.lfo_depth, 0.0), 0.0, 1.0);
        p.grain_size = clamp(finite(p.grain_size, 0.08), 0.002, 0.5);
        p.grain_position = clamp(finite(p.grain_position, 0.5), 0.0, 1.0);
        p.grain_density = if p.grain_density >= 1 && p.grain_density <= ST_GRANULAR_MAX_GRAINS as i32 {
            p.grain_density
        } else {
            ST_GRANULAR_DEFAULT_DENSITY
        };
        p.grain_spray = clamp(finite(p.grain_spray, 0.0), 0.0, 1.0);
        p.grain_pitch_spread = clamp(finite(p.grain_pitch_spread, 0.0), 0.0, 24.0);
        p.grain_scan = clamp(finite(p.grain_scan, 0.0), -4.0, 4.0);
        p.grain_reverse = clamp(finite(p.grain_reverse, 0.0), 0.0, 1.0);
        p.grain_stereo = clamp(finite(p.grain_stereo, 0.0), 0.0, 1.0);
        p.filter_attack = clamp(finite(p.filter_attack, 0.005), 0.0, 60.0);
        p.filter_decay = clamp(finite(p.filter_decay, 0.2), 0.0, 60.0);
        p.filter_sustain = clamp(finite(p.filter_sustain, 0.0), 0.0, 1.0);
        p.filter_release = clamp(finite(p.filter_release, 0.2), 0.0001, 60.0);
        p.filter_env_amount = clamp(finite(p.filter_env_amount, 0.0), -48.0, 48.0);
        p.lfo_target = clamp(p.lfo_target, 0, 5);
        p.lfo2_target = clamp(p.lfo2_target, 0, 5);
        p.lfo_shape = clamp(p.lfo_shape, 0, 4);
        p.lfo2_shape = clamp(p.lfo2_shape, 0, 4);
        p.lfo2_rate = clamp(finite(p.lfo2_rate, 0.0), 0.0, 100.0);
        p.lfo2_depth = clamp(finite(p.lfo2_depth, 0.0), 0.0, 1.0);
        p.lfo_sync = (p.lfo_sync != 0) as i32;
        p.lfo2_sync = (p.lfo2_sync != 0) as i32;
        p.lfo_beats = clamp(finite(p.lfo_beats, 1.0), 1.0 / 64.0, 64.0);
        p.lfo2_beats = clamp(finite(p.lfo2_beats, 1.0), 1.0 / 64.0, 64.0);
        if let Some(slot) = self.edit().instruments.get_mut(index as usize) {
            slot.p = p;
        }
    }

    /// The song: up to 4096 pattern indices, whether it loops, and the pattern Pattern mode plays.
    pub fn set_sequence(&mut self, patterns: &[i32], loop_song: bool, active_pattern: i32) {
        let p = self.edit();
        let last = p.patterns.len() as i32 - 1;
        p.sequence.clear();
        p.sequence.extend(patterns.iter().take(4096).map(|&index| clamp(index, 0, last)));
        if p.sequence.is_empty() {
            p.sequence.push(0);
        }
        p.loop_song = loop_song;
        p.active_pattern = clamp(active_pattern, 0, last);
    }

    /// Loops a section of the song sequence (inclusive indices); a negative start, or an end before
    /// the start, removes the loop.
    pub fn set_song_loop(&mut self, mut start_index: i32, mut end_index: i32) {
        if start_index < 0 || end_index < start_index {
            start_index = -1;
            end_index = -1;
        }
        let p = self.edit();
        p.song_loop_start = cmin(start_index, 4095);
        p.song_loop_end = cmin(end_index, 4095);
    }

    /// The host's identifier for its current numbering of automation destinations; host parameter
    /// events carry it (`STEvent::host_map`).
    pub fn set_host_automation_map(&mut self, map: u64) {
        self.edit().host_map = map;
    }

    // ---- patterns, steps and automation ----

    /// Creates or resizes pattern `index` (length clamped to 1..=`ST_MAX_ROWS` rows).
    pub fn set_pattern(&mut self, index: i32, length: i32) {
        if index < 0 || index >= ST_MAX_PATTERNS {
            return;
        }
        let p = self.edit_patterns();
        let index = index as usize;
        if index >= p.patterns.len() {
            p.patterns.resize(index + 1, Pattern::default());
        }
        p.patterns[index].resize(clamp(length, 1, ST_MAX_ROWS));
    }

    /// Share pattern `index` with the last published project.
    pub fn reuse_pattern(&mut self, index: i32) -> bool {
        if index < 0 || index >= ST_MAX_PATTERNS {
            return false;
        }
        let index = index as usize;
        let Some(published) = self.control.patterns.get(index).cloned() else { return false };
        let p = self.edit_patterns();
        if index >= p.patterns.len() {
            p.patterns.resize(index + 1, Pattern::default());
        }
        p.patterns[index] = published;
        true
    }

    /// The identity of the published pattern content: a commit that publishes pattern edits gives
    /// it a new, process-unique value; a commit without pattern edits keeps it.
    pub fn pattern_epoch(&self) -> u64 {
        self.control.pattern_epoch
    }

    /// Writes column 0 of a row: note, instrument, velocity, gate, both step effects and an
    /// optional chord (up to `ST_MAX_CHORD` semitone offsets; `None` or an empty list for a single
    /// note). Rows and tracks out of range are ignored.
    pub fn set_step(&mut self, pattern: i32, track: i32, row: i32, step: STStep, chord: Option<&[i32]>) {
        if pattern < 0 || track < 0 || track >= ST_TRACK_SANITY_LIMIT || row < 0 {
            return;
        }
        let p = self.edit_patterns();
        let instruments = p.instruments.len();
        let Some(target) = p.patterns.get_mut(pattern as usize) else { return };
        if row >= target.length {
            return;
        }
        let mut s = step;
        s.note = clamp(s.note, -3, 127);
        sanitize_step_note(&mut s, instruments);
        s.fx1.value = finite(s.fx1.value, 0.0);
        s.fx2.value = finite(s.fx2.value, 0.0);
        let v = target.mutable_step(track, row);
        v.plugin_fx = [PluginStepFX::NONE; 2];
        v.present = true;
        v.p = s;
        (v.count, v.chord) = sanitize_chord(chord);
    }

    /// A parameter lock on a row that has a step: for the row, the host destination's parameter
    /// at `address` takes the normalized `value` (0..=1) instead of what its host lane says.
    /// `column` is 0 or 1, the step effect column the lock stands in. False when an argument is
    /// out of range or the row has no step.
    pub fn set_plugin_step_fx(
        &mut self,
        pattern: i32,
        track: i32,
        row: i32,
        column: i32,
        host_destination: i32,
        address: u64,
        value: f64,
    ) -> bool {
        if pattern < 0
            || track < 0
            || track >= ST_TRACK_SANITY_LIMIT
            || row < 0
            || column < 0
            || column > 1
            || host_destination < 0
            || host_destination >= ST_MAX_HOST_DESTINATIONS
            || !value.is_finite()
            || value < 0.0
            || value > 1.0
        {
            return false;
        }
        let p = self.edit_patterns();
        let Some(target) = p.patterns.get_mut(pattern as usize) else { return false };
        if row >= target.length || !target.step(track, row).present {
            return false;
        }
        target.mutable_step(track, row).plugin_fx[column as usize] =
            PluginStepFX { present: true, destination: host_destination, address, value };
        true
    }

    /// Writes a note column of a row. Column 0 is `set_step` without a chord; the extra columns
    /// (1..`ST_MAX_NOTE_COLUMNS`) hold note, instrument, velocity and gate only, and a note outside
    /// -2..=127 clears the cell.
    pub fn set_step_column(&mut self, pattern: i32, track: i32, row: i32, column: i32, step: STStep) {
        if pattern < 0 || track < 0 || track >= ST_TRACK_SANITY_LIMIT || row < 0 || column < 0 || column > EXTRA_COLUMNS {
            return;
        }
        if column == 0 {
            self.set_step(pattern, track, row, step, None);
            return;
        }
        let p = self.edit_patterns();
        let instruments = p.instruments.len();
        let Some(target) = p.patterns.get_mut(pattern as usize) else { return };
        if row >= target.length {
            return;
        }
        let mut s = step;
        if s.note < -2 || s.note > 127 {
            if target.column_step(track, column, row).present {
                *target.mutable_column_step(track, column, row) = Step::BLANK;
            }
            return;
        }
        sanitize_step_note(&mut s, instruments);
        s.fx1 = STStepFX::default();
        s.fx2 = STStepFX::default();
        let v = target.mutable_column_step(track, column, row);
        *v = Step::BLANK;
        v.present = true;
        v.p = s;
    }

    /// Adds a point to the lane of a track's own parameter (`ST_TARGET_*`: the track's mixer
    /// values, its filter and pitch offsets, its instrument's parameters), creating the lane on
    /// first use (at most 256 lanes per pattern and 4096 points per lane). `interpolation` is 1
    /// for a stepped lane, 0 for a linear one.
    pub fn add_automation_point(&mut self, pattern: i32, track: i32, target: i32, interpolation: i32, row: f64, value: f64) {
        if pattern < 0
            || track < 0
            || track >= channel_count(ST_TRACK_SANITY_LIMIT)
            || target < 0
            || target > ST_TARGET_LAST_INSTRUMENT
            || (target > ST_TARGET_PITCH && target < ST_TARGET_ATTACK)
            || !row.is_finite()
            || !value.is_finite()
        {
            return;
        }
        let p = self.edit_patterns();
        let Some(pattern) = p.patterns.get_mut(pattern as usize) else { return };
        add_lane_point(
            pattern,
            false,
            |lane| lane.track == track && lane.target == target,
            Lane { track, target, interpolation, address: 0, points: Vec::new() },
            row,
            value,
        );
    }

    /// Adds a point to the host lane `(destination, address)` of a pattern; the engine reports
    /// its values as `ST_EVENT_HOST_PARAMETER` events and applies them to nothing itself.
    pub fn add_host_automation_point(
        &mut self,
        pattern: i32,
        destination: i32,
        address: u64,
        interpolation: i32,
        row: f64,
        value: f64,
    ) {
        if pattern < 0 || destination < 0 || destination >= ST_MAX_HOST_DESTINATIONS || !row.is_finite() || !value.is_finite()
        {
            return;
        }
        let p = self.edit_patterns();
        let Some(pattern) = p.patterns.get_mut(pattern as usize) else { return };
        add_lane_point(
            pattern,
            true,
            |lane| lane.target == destination && lane.address == address,
            Lane {
                track: -1,
                target: destination,
                interpolation: if interpolation == 1 { 1 } else { 0 },
                address,
                points: Vec::new(),
            },
            row,
            value,
        );
    }

    // ---- samples, zones and timeline audio ----

    /// `None` clears the instrument's sample. Publishes at once outside
    /// an update.
    pub fn set_sample(&mut self, sh: &Shared, instrument: i32, pcm: Option<Pcm>) -> bool {
        if !self.view().has_instrument(instrument) {
            return false;
        }
        let index = instrument as usize;
        let reference = match pcm {
            None => None,
            Some(pcm) => {
                if !pcm.valid() {
                    return false;
                }
                let Some(sample) = pcm.to_sample() else { return false };
                self.adopt_sample(sample)
            }
        };
        self.edit().instruments[index].set_sample(reference);
        self.publish_outside_update(sh)
    }

    /// An empty list clears the manual slices.
    pub fn set_slice_points(&mut self, instrument: i32, positions: &[f32]) {
        if !self.view().has_instrument(instrument) {
            return;
        }
        let slices = &mut self.edit().instruments[instrument as usize].slices;
        slices.clear();
        if !positions.is_empty() {
            slices.push(0.0);
            slices.extend(positions.iter().take(128).copied().filter(|x| x.is_finite() && *x > 0.0 && *x < 1.0));
            slices.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            slices.dedup();
        }
    }

    /// Frames per wavetable cycle of an instrument's sample (0, or
    /// `ST_WAVETABLE_MIN_FRAME`..=`ST_WAVETABLE_MAX_FRAME`). Publishes at once outside an update.
    pub fn set_sample_cycle(&mut self, sh: &Shared, instrument: i32, frames: i32) -> bool {
        if !self.view().has_instrument(instrument)
            || frames < 0
            || (frames > 0 && (frames < ST_WAVETABLE_MIN_FRAME || frames > ST_WAVETABLE_MAX_FRAME))
        {
            return false;
        }
        self.edit().instruments[instrument as usize].cycle_frames = frames;
        self.publish_outside_update(sh)
    }

    /// Removes every key/velocity zone of an instrument.
    pub fn clear_zones(&mut self, instrument: i32) {
        if self.view().has_instrument(instrument) {
            self.edit().instruments[instrument as usize].zones.clear();
        }
    }

    /// Adds a key/velocity zone with its own PCM, root note, tuning and gain (up to 128 per
    /// instrument). False for PCM the engine rejects or a full zone list. Publishes at once outside
    /// an update.
    #[allow(clippy::too_many_arguments)]
    pub fn add_sample_zone(
        &mut self,
        sh: &Shared,
        instrument: i32,
        pcm: Pcm,
        key_low: i32,
        key_high: i32,
        velocity_low: i32,
        velocity_high: i32,
        root_note: i32,
        tune: f32,
        volume: f32,
    ) -> bool {
        if !self.view().has_instrument(instrument) || !pcm.valid() {
            return false;
        }
        let index = instrument as usize;
        if self.edit().instruments[index].zones.len() >= 128 {
            return false;
        }
        let Some(sample) = pcm.to_sample() else { return false };
        let mut zone = Zone { sample: self.adopt_sample(sample), ..Zone::default() };
        zone.key_low = clamp(key_low, 0, 127);
        zone.key_high = clamp(key_high, zone.key_low, 127);
        zone.velocity_low = clamp(velocity_low, 0, 127);
        zone.velocity_high = clamp(velocity_high, zone.velocity_low, 127);
        zone.root_note = clamp(root_note, 0, 127);
        zone.tune = clamp(finite(tune, 0.0), -96.0, 96.0);
        zone.volume = clamp(finite(volume, 1.0), 0.0, 2.0);
        self.edit().instruments[index].zones.push(zone);
        self.publish_outside_update(sh)
    }

    /// `None` clears the slot. Publishes at once outside an update.
    pub fn set_audio_asset(&mut self, sh: &Shared, asset: i32, pcm: Option<Pcm>) -> bool {
        if asset < 0 || asset >= AUDIO_ASSETS as i32 {
            return false;
        }
        let index = asset as usize;
        self.edit();
        let reference = match pcm {
            None => None,
            Some(pcm) => {
                if !pcm.valid() {
                    return false;
                }
                let Some(sample) = pcm.to_sample() else { return false };
                self.adopt_sample(sample)
            }
        };
        self.edit().audio_assets[index].sample = reference;
        self.publish_outside_update(sh)
    }

    /// Song clips, pattern clips and song tail clips.
    pub fn clear_audio_clips(&mut self) {
        let p = self.edit();
        p.audio_clips.clear();
        p.pattern_clips.clear();
        p.song_tail_clips.clear();
    }

    /// Places a timeline asset on a track: start and duration in quarter beats, source offset in
    /// seconds, gain, fades in seconds and mute. At most 256 clips; further ones are ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn add_audio_clip(
        &mut self,
        asset: i32,
        track: i32,
        start_beat: f64,
        duration_beats: f64,
        source_offset: f64,
        gain: f32,
        fade_in: f32,
        fade_out: f32,
        muted: bool,
    ) {
        if asset < 0 || asset >= AUDIO_ASSETS as i32 || track < 0 || track >= ST_TRACK_SANITY_LIMIT {
            return;
        }
        let p = self.edit();
        if p.audio_clips.len() >= 256 {
            return;
        }
        p.audio_clips.push(AudioClip {
            asset,
            track,
            start_beat: clamp(finite64(start_beat, 0.0), 0.0, 1000000.0),
            duration_beats: clamp(finite64(duration_beats, 0.0), 0.0, 1000000.0),
            source_offset: clamp(finite64(source_offset, 0.0), 0.0, 1000000.0),
            gain: clamp(finite(gain, 1.0), 0.0, 4.0),
            fade_in: clamp(finite(fade_in, 0.005), 0.0, 3600.0),
            fade_out: clamp(finite(fade_out, 0.005), 0.0, 3600.0),
            muted,
        });
    }

    /// A Pattern-mode clip: the printed head and loop assets of one pattern on a track.
    pub fn add_pattern_clip(&mut self, head_asset: i32, loop_asset: i32, track: i32, pattern: i32, gain: f32) {
        if head_asset < 0
            || head_asset >= AUDIO_ASSETS as i32
            || loop_asset >= AUDIO_ASSETS as i32
            || track < 0
            || track >= ST_TRACK_SANITY_LIMIT
            || pattern < 0
            || pattern >= ST_MAX_PATTERNS
        {
            return;
        }
        let p = self.edit();
        if p.pattern_clips.len() >= 256 {
            return;
        }
        p.pattern_clips.push(PatternClip {
            head_asset,
            loop_asset: if loop_asset < 0 { -1 } else { loop_asset },
            track,
            pattern,
            gain: clamp(finite(gain, 1.0), 0.0, 4.0),
        });
    }

    /// A song-loop tail: an asset on a track with the song frame at which it starts (see
    /// `SongTailClip`). At most 64.
    pub fn add_song_tail_clip(&mut self, asset: i32, track: i32, start_frame: i64, gain: f32) {
        if asset < 0 || asset >= AUDIO_ASSETS as i32 || track < 0 || track >= ST_TRACK_SANITY_LIMIT || start_frame < 0 {
            return;
        }
        let p = self.edit();
        if p.song_tail_clips.len() >= 64 {
            return;
        }
        p.song_tail_clips.push(SongTailClip { asset, track, start: start_frame, gain: clamp(finite(gain, 1.0), 0.0, 4.0) });
    }

    /// The published song's length in seconds, or the active pattern's when `song_mode` is
    /// false. Excludes release tails.
    pub fn duration(&self, song_mode: bool) -> f64 {
        let p = &self.control;
        if song_mode {
            return p.song_duration();
        }
        p.pattern_rows(p.active_pattern) * 60.0 / p.bpm / p.lpb as f64
    }

    // ---- commands (the control thread is the command ring's single producer) ----

    /// Queues Play: of the song sequence (`song_mode`) or of the active pattern. Like every
    /// command below it takes effect when the render half starts its next block; until then the
    /// status reports it as pending (`STStatus::transport_playing`).
    pub fn play(&mut self, sh: &Shared, song_mode: bool) {
        self.queue_transport(sh, Command::new(CMD_PLAY, song_mode as i32, 0, 0, 0, 0.0), true);
    }

    /// Queues Stop.
    pub fn stop(&mut self, sh: &Shared) {
        self.queue_transport(sh, Command::new(CMD_STOP, 0, 0, 0, 0, 0.0), false);
    }

    /// Queues Play after a count-in of 1..=4 bars.
    pub fn play_count_in(&mut self, sh: &Shared, song_mode: bool, bars: i32) {
        self.queue_transport(sh, Command::new(CMD_COUNT_IN, song_mode as i32, clamp(bars, 1, 4), 0, 0, 0.0), false);
    }

    /// Queues the recording flag of the transport (`STStatus::recording`,
    /// `ST_INPUT_CLOCK_RECORDING` on input acknowledgements).
    pub fn set_recording(&mut self, sh: &Shared, enabled: bool) {
        self.queue(sh, &Command::new(CMD_RECORDING, enabled as i32, 0, 0, 0, 0.0));
    }

    /// Queues a seek to a row of a pattern, at an entry of the song sequence.
    pub fn seek(&mut self, sh: &Shared, pattern: i32, row: i32, sequence: i32) {
        self.queue(sh, &Command::new(CMD_SEEK, pattern, row, sequence, 0, 0.0));
    }

    /// Queues a row loop for Pattern mode (inclusive rows); a negative start, or an end before the
    /// start, removes the loop.
    pub fn set_pattern_loop(&mut self, sh: &Shared, mut start_row: i32, mut end_row: i32) {
        if start_row < 0 || end_row < start_row {
            start_row = -1;
            end_row = -1;
        }
        self.queue(sh, &Command::new(
            CMD_PATTERN_LOOP,
            cmin(start_row, ST_MAX_ROWS - 1),
            cmin(end_row, ST_MAX_ROWS - 1),
            0,
            0,
            0.0,
        ));
    }

    /// Queues a jump to another pattern or sequence entry, which the transport takes when the
    /// current pattern ends (`STStatus::queued_pattern`, `queued_sequence`; -1 for "none").
    pub fn queue_jump(&mut self, sh: &Shared, pattern: i32, sequence: i32) {
        self.queue(sh, &Command::new(
            CMD_QUEUE_JUMP,
            if pattern < 0 { -1 } else { cmin(pattern, ST_MAX_PATTERNS - 1) },
            if sequence < 0 { -1 } else { cmin(sequence, 4095) },
            0,
            0,
            0.0,
        ));
    }

    /// Queues a switch between Song and Pattern mode, continuing at the given pattern or sequence
    /// entry (-1 for "none").
    pub fn queue_mode(&mut self, sh: &Shared, song_mode: bool, pattern: i32, sequence: i32) {
        self.queue(sh, &Command::new(
            CMD_QUEUE_MODE,
            song_mode as i32,
            if pattern < 0 { -1 } else { cmin(pattern, ST_MAX_PATTERNS - 1) },
            if sequence < 0 { -1 } else { cmin(sequence, 4095) },
            0,
            0.0,
        ));
    }

    /// Queues a live note (keyboard, MIDI input) on a track with an instrument.
    pub fn note_on(&mut self, sh: &Shared, track: i32, instrument: i32, note: i32, velocity: i32) {
        self.queue(sh, &Command::new(CMD_NOTE_ON, track, instrument, note, velocity, 0.0));
    }

    /// Queues the release of the live note of this pitch on a track.
    pub fn note_off(&mut self, sh: &Shared, track: i32, note: i32) {
        self.queue(sh, &Command::new(CMD_NOTE_OFF, track, note, 0, 0, 0.0));
    }

    /// Queues a channel message played live for an instrument the engine does not sound
    /// itself: a controller (the pedals among them), pressure, pitch bend or a program
    /// change, for the plug-in instrument of `track` (`instrument` -1) or the external MIDI
    /// instrument `instrument` (`track` -1). The engine does nothing with it but hand it on
    /// as an `ST_EVENT_MIDI` event, in its place among the live notes queued before and
    /// after it: one stream, in the order of the calls. False when the queue is full.
    pub fn midi(&mut self, sh: &Shared, track: i32, instrument: i32, status: u8, data1: u8, data2: u8) -> bool {
        if !self.owns(sh) {
            return false;
        }
        if sh.commands.available() == 0 {
            sh.dropped_commands.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let data = i32::from(data1 & 0x7F) | i32::from(data2 & 0x7F) << 8;
        self.queue(sh, &Command::new(CMD_MIDI, track, instrument, i32::from(status & 0xF0), data, 0.0));
        true
    }

    /// Queues a note edge of a live input (`edge`: `ST_INPUT_NOTE_ON` or `ST_INPUT_NOTE_OFF`) to
    /// be acknowledged with the position it lands on (`STInputAck`, by `request_id`, which must
    /// grow from edge to edge); with `apply_note` the note is also played or released. True only
    /// when both command and ACK capacity are reserved.
    #[allow(clippy::too_many_arguments)]
    pub fn input_note(
        &mut self,
        sh: &Shared,
        request_id: u64,
        edge: i32,
        track: i32,
        instrument: i32,
        note: i32,
        velocity: i32,
        apply_note: bool,
    ) -> bool {
        if request_id == 0
            || request_id <= self.last_input_request_id
            || (edge != ST_INPUT_NOTE_ON && edge != ST_INPUT_NOTE_OFF)
            || track < 0
            || track >= self.control.channels()
            || !self.view().has_instrument(instrument)
            || note < 0
            || note > 127
            || velocity < 0
            || velocity > 127
            || (edge == ST_INPUT_NOTE_ON && velocity == 0)
        {
            return false;
        }
        if !self.owns(sh) {
            return false;
        }
        let outstanding = sh.input_outstanding.fetch_add(1, Ordering::Relaxed);
        if outstanding >= ST_INPUT_ACK_CAPACITY as u32 - 1 {
            sh.input_outstanding.fetch_sub(1, Ordering::Relaxed);
            return false;
        }
        let command = Command {
            kind: CMD_INPUT_NOTE,
            a: track,
            b: instrument,
            c: note,
            d: velocity,
            request_id,
            input_edge: edge,
            apply_input_note: apply_note as i32,
            ..Command::default()
        };
        if !sh.commands.push(&command) {
            sh.input_outstanding.fetch_sub(1, Ordering::Relaxed);
            sh.dropped_commands.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.last_input_request_id = request_id;
        true
    }

    /// Queues an audition of a step (with an optional chord) for 0.03..2 seconds, without moving
    /// the transport.
    pub fn preview_step(&mut self, sh: &Shared, track: i32, step: STStep, chord: Option<&[i32]>, duration_seconds: f32) {
        let mut command = Command { kind: CMD_PREVIEW, a: track, step, ..Command::default() };
        command.step.note = clamp(step.note, -3, 127);
        command.step.instrument = clamp(step.instrument, -1, self.view().instruments.len() as i32 - 1);
        command.step.velocity = clamp(step.velocity, 0, 127);
        command.value = clamp(finite(duration_seconds, 0.35), 0.03, 2.0);
        (command.chord_count, command.chord) = sanitize_chord(chord);
        self.queue(sh, &command);
    }

    /// Queues Panic: every voice and pending note ends.
    pub fn panic(&mut self, sh: &Shared) {
        self.queue(sh, &Command::new(CMD_PANIC, 0, 0, 0, 0, 0.0));
    }

    /// Queues a live pitch bend of a track, in semitones (clamped to +-48).
    pub fn pitch_bend(&mut self, sh: &Shared, track: i32, semitones: f32) {
        self.queue(sh, &Command::new(CMD_PITCH_BEND, track, 0, 0, 0, clamp(finite(semitones, 0.0), -48.0, 48.0)));
    }

    /// Queues the live modulation wheel of a track (clamped to 0..1).
    pub fn mod_wheel(&mut self, sh: &Shared, track: i32, value: f32) {
        self.queue(sh, &Command::new(CMD_MOD_WHEEL, track, 0, 0, 0, clamp(finite(value, 0.0), 0.0, 1.0)));
    }

    /// False for an invalid instrument/target or a full
    /// queue.
    pub fn set_instrument_parameter(&mut self, sh: &Shared, instrument: i32, target: i32, value: f32) -> bool {
        if instrument < 0 || instrument >= ST_INSTRUMENT_SANITY_LIMIT || !value.is_finite() {
            return false;
        }
        if !(target == ST_TARGET_CUTOFF
            || target == ST_TARGET_RESONANCE
            || (target >= ST_TARGET_ATTACK && target <= ST_TARGET_LAST_INSTRUMENT))
        {
            return false;
        }
        if !self.owns(sh) {
            return false;
        }
        if sh.commands.available() == 0 {
            sh.dropped_commands.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.queue(sh, &Command::new(CMD_LIVE_PARAM, instrument, target, 0, 0, value));
        true
    }
}

//! The tracker sequencer of `STEngine` in `TrackerEngine.cpp`: row triggering (`triggerRow`,
//! `triggerStep`, `triggerColumns`), step effects and row locks (`applyFX`), delayed hits and
//! ratchets (`schedule*`, `repeatRatchet`), plug-in parameter locks, and automation lanes
//! (`laneValue`, `emitLaneAutomation`, `automation`, `emitMixState`).

use super::abi::*;
use super::project::{lane_track, Lane, Pattern, PluginStepFX, Project, Step, EXTRA_COLUMNS};
use super::state::*;
use super::tables::*;
use super::util::{clamp, cmax, finite, lowest_bit, random_bipolar};
use super::voice::NoteOn;
use std::sync::atomic::Ordering;

/// Note FX of a row (column 0's FX 1/FX 2), shared by every note column of the track on that row.
#[derive(Clone, Copy)]
struct RowFX {
    probability: f32,
    offset: f32,
    micro: f32,
    glide: f32,
    ratchet: i32,
    reverse: i32,
    arp: i32,
    slice: i32,
    /// -1 not rolled yet, else the row's single probability roll.
    roll: i32,
}

impl RowFX {
    const NONE: RowFX =
        RowFX { probability: 1.0, offset: 0.0, micro: 0.0, glide: 0.0, ratchet: 1, reverse: -1, arp: 0, slice: -1, roll: -1 };
}

/// One note (or chord) to schedule over a row: `scheduleHits`' arguments.
struct Hits<'s> {
    lane: i32,
    channel: i32,
    instrument: i32,
    step: &'s STStep,
    count: i32,
    chord: &'s [i32; ST_MAX_CHORD],
    ratchet: i32,
    micro: f32,
    offset: f32,
    reverse: i32,
    glide: f32,
    arp: i32,
    slice: i32,
}

const UNISON: [i32; ST_MAX_CHORD] = [0; ST_MAX_CHORD];

/// `laneValue`: the lane's value at pattern position `pos` (rows), or at `musical_beat` for a
/// stepped lane under a timing map.
fn lane_value(p: &Project, musical_beat: f64, row_base_beat: f64, l: &Lane, mut pos: f64) -> f64 {
    let points = &l.points;
    let (Some(front), Some(back)) = (points.first(), points.last()) else { return 0.0 };
    if p.timing.is_some() && l.interpolation == 1 {
        // upper_bound: the first point whose mapped beat lies after the current beat.
        let next = points.partition_point(|point| {
            let whole = point.row.floor() as i32;
            let start = p.beat_for_row(whole);
            let end = p.beat_for_row(whole.wrapping_add(1));
            !(musical_beat < row_base_beat + start + (point.row - whole as f64) * (end - start))
        });
        return if next == 0 { front.value } else { points[next - 1].value };
    }
    if l.interpolation == 1 {
        pos += 1e-10;
    }
    if pos <= front.row {
        return front.value;
    }
    let it = points.partition_point(|point| !(pos < point.row));
    if it >= points.len() {
        return back.value;
    }
    if it == 0 {
        return front.value; // Unreachable: pos > front.row, so the first point is not after pos.
    }
    let a = points[it - 1];
    let b = points[it];
    if l.interpolation == 1 || b.row == a.row {
        return a.value;
    }
    let result = a.value + (b.value - a.value) * (pos - a.row) / (b.row - a.row);
    if result.is_finite() {
        return result;
    }
    let fraction = (pos - a.row) / (b.row - a.row);
    a.value * (1.0 - fraction) + b.value * fraction
}

/// `emitLaneAutomation`: stepped lanes emit each change at its exact sample (retrying dropped
/// deliveries every 32 frames); linear lanes emit on the 32-frame control interval.
#[allow(clippy::too_many_arguments)]
fn emit_lane_automation(
    out: &EventOut,
    sh: &Shared,
    frame: u64,
    host_map: u64,
    l: &Lane,
    state: &mut LaneEventState,
    value: f64,
    event_type: i32,
) {
    let first = !state.known;
    let stepped = l.interpolation == 1;
    if first || (stepped && value != state.value) {
        state.known = true;
        state.value = value;
        state.delivered = false;
        state.retry_frame = frame;
    }
    let due = if stepped {
        !state.delivered && frame >= state.retry_frame
    } else {
        first || (frame & 31) == 0 || (!state.delivered && frame >= state.retry_frame)
    };
    if due {
        let event = STEvent {
            type_: event_type,
            track: l.track,
            target: l.target,
            slot: l.slot,
            address: l.address,
            value,
            hostMap: if event_type == ST_EVENT_HOST_PARAMETER { host_map } else { 0 },
            ..STEvent::default()
        };
        state.delivered = emit_event(out, sh, event);
        state.retry_frame = frame.wrapping_add(32);
        if state.delivered {
            state.value = value;
        }
    }
}

impl<'a> Rt<'a> {
    #[inline]
    pub fn emit(&mut self, event: STEvent) -> bool {
        emit_event(&self.e.out, self.sh, event)
    }

    pub fn reset_automation_events(&mut self) {
        self.e.lane_event_state = [LaneEventState::default(); 256];
        self.e.host_lane_event_state = [LaneEventState::default(); 256];
        self.e.automation_pattern = -1;
    }

    /// `projectChannelEnabled(*current, channel)`
    #[inline(always)]
    pub fn channel_enabled(&self, channel: i32) -> bool {
        channel >= 0 && channel < self.p.track_count
    }

    /// Timed defaults for track volume/pan/sends and the assigned instrument's volume/pan on
    /// every configured track; active lanes override them at the same sample in FIFO order.
    pub fn emit_mix_state(&mut self) {
        let p = self.p;
        for t in 0..p.track_count {
            let (Some(s), Some(mix)) = (self.tb.track_state.get(t as usize), p.tracks.get(t as usize)) else { continue };
            let Some(instrument) = p.instruments.get(mix.instrument as u32 as usize) else { continue };
            let i = &instrument.p;
            let instrument_volume = s.instrument[(ST_TARGET_INSTRUMENT_VOLUME - ST_TARGET_ATTACK) as usize];
            let instrument_pan = s.instrument[(ST_TARGET_INSTRUMENT_PAN - ST_TARGET_ATTACK) as usize];
            let targets = [
                ST_TARGET_VOLUME,
                ST_TARGET_PAN,
                ST_TARGET_DELAY,
                ST_TARGET_REVERB,
                ST_TARGET_INSTRUMENT_VOLUME,
                ST_TARGET_INSTRUMENT_PAN,
            ];
            let values = [
                s.volume,
                s.pan,
                s.delay,
                s.reverb,
                if instrument_volume.is_finite() { instrument_volume } else { i.volume },
                if instrument_pan.is_finite() { instrument_pan } else { i.pan },
            ];
            for n in 0..6 {
                let event = STEvent {
                    type_: ST_EVENT_PARAMETER,
                    track: t,
                    target: targets[n],
                    value: values[n] as f64,
                    ..STEvent::default()
                };
                emit_event(&self.e.out, self.sh, event);
            }
        }
    }

    pub fn clear_plugin_locks(&mut self) {
        self.e.plugin_row_lock_count = 0;
        self.e.plugin_locks_pending = 0;
        self.e.plugin_lane_locked = [false; 256];
        self.e.plugin_host_lane_locked = [false; 256];
    }

    pub fn clear_ratchets(&mut self) {
        self.tb.lane_ratchets.fill(RatchetState::default());
    }

    /// Reset every channel's effective mixer values to the project's and (optionally) emit them.
    pub fn reset_track_state(&mut self, emit_defaults: bool) {
        self.clear_plugin_locks();
        self.reset_automation_events();
        let channels = self.tb.channels as usize;
        for (state, mix) in self.tb.track_state.iter_mut().zip(self.p.tracks.iter()).take(channels) {
            *state = TrackState::default();
            state.volume = mix.volume;
            state.pan = mix.pan;
            state.delay = mix.delaySend;
            state.reverb = mix.reverbSend;
        }
        if emit_defaults {
            self.emit_mix_state();
        }
    }

    pub fn consume_early_input(&mut self, lane: i32, note: i32, row: i32) -> bool {
        let pattern = self.e.pattern;
        for entry in self.e.early_inputs.iter_mut() {
            if entry.track == lane && entry.note == note && entry.pattern == pattern && entry.row == row {
                *entry = EarlyInput::NONE;
                return true;
            }
        }
        false
    }

    pub fn forget_early_inputs(&mut self) {
        self.e.early_inputs = [EarlyInput::NONE; 16];
    }

    pub fn clear_scheduled(&mut self) {
        for s in self.e.scheduled.iter_mut() {
            s.active = false;
        }
        self.e.scheduled_masks = [0; SCHEDULED_GROUPS];
        self.e.scheduled_count = 0;
        self.e.scheduled_groups_used = 0;
    }

    /// Lowest free slot of the schedule, marked taken.
    fn scheduled_slot(&mut self) -> Option<&mut Scheduled> {
        let e = &mut *self.e;
        for group in 0..SCHEDULED_GROUPS {
            let available = !e.scheduled_masks[group];
            if available != 0 {
                let bit = lowest_bit(available);
                e.scheduled_masks[group] |= 1u64 << bit;
                e.scheduled_groups_used = cmax(e.scheduled_groups_used, group + 1);
                return e.scheduled.get_mut(group * 64 + bit);
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_mapped(
        &mut self,
        track: i32,
        instrument: i32,
        note: i32,
        velocity: i32,
        onset: f64,
        end: f64,
        hits: &Hits,
    ) {
        let loop_beat_offset = self.e.loop_beat_offset;
        if let Some(s) = self.scheduled_slot() {
            *s = Scheduled::NEW;
            s.active = true;
            s.mapped = true;
            s.onset_beat = onset + loop_beat_offset;
            s.end_beat = end + loop_beat_offset;
            s.track = track;
            s.instrument = instrument;
            s.note = note;
            s.velocity = velocity;
            s.offset = hits.offset;
            s.reverse = hits.reverse;
            s.glide = hits.glide;
            s.arp = hits.arp;
            s.slice = hits.slice;
            s.sequence_track = hits.lane;
            s.open = end < 0.0;
            self.e.scheduled_count += 1;
            return;
        }
        self.sh.dropped_events.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule(&mut self, track: i32, instrument: i32, note: i32, velocity: i32, delay: f64, gate: f64, hits: &Hits) {
        let total_frames = self.e.total_frames;
        if let Some(s) = self.scheduled_slot() {
            *s = Scheduled {
                active: true,
                due: total_frames + delay.round(),
                gate: gate.round(),
                track,
                instrument,
                note,
                velocity,
                offset: hits.offset,
                glide: hits.glide,
                reverse: hits.reverse,
                arp: hits.arp,
                slice: hits.slice,
                sequence_track: hits.lane,
                ..Scheduled::NEW
            };
            self.e.scheduled_count += 1;
            return;
        }
        self.sh.dropped_events.fetch_add(1, Ordering::Relaxed);
    }

    /// A step effect that acts on the channel: sets the row's value, locks the matching
    /// automation target for this row and reports it to the host.
    fn apply_fx(&mut self, track: i32, fx: STStepFX) {
        let Some(t) = self.tb.track_state.get_mut(track as u32 as usize) else { return };
        let x = finite(fx.value, 0.0);
        let mut target = -1;
        match fx.type_ {
            ST_FX_VOLUME => {
                t.volume = clamp(x, 0.0, 2.0);
                target = ST_TARGET_VOLUME;
            }
            ST_FX_PAN => {
                t.pan = clamp(x, -1.0, 1.0);
                target = ST_TARGET_PAN;
            }
            ST_FX_PITCH => {
                t.pitch = clamp(x, -48.0, 48.0);
                target = ST_TARGET_PITCH;
            }
            ST_FX_CUTOFF => {
                t.cutoff = clamp(x, 20.0, 22000.0);
                target = ST_TARGET_CUTOFF;
            }
            ST_FX_RESONANCE => {
                t.resonance = clamp(x, 0.0, 0.99);
                target = ST_TARGET_RESONANCE;
            }
            ST_FX_DELAY => {
                t.delay = clamp(x, 0.0, 1.0);
                target = ST_TARGET_DELAY;
            }
            ST_FX_REVERB => {
                t.reverb = clamp(x, 0.0, 1.0);
                target = ST_TARGET_REVERB;
            }
            ST_FX_POSITION => {
                // Wavetable position for this track's voices, as a row lock on the automation
                // target. Granular: the same value is the grain position.
                let slot = (ST_TARGET_WAVETABLE_POSITION - ST_TARGET_ATTACK) as usize;
                t.instrument[slot] = clamp(x, 0.0, 1.0);
                t.instrument_mask |= 1u64 << slot;
                t.row_locks |= 1u64 << ST_TARGET_WAVETABLE_POSITION;
                let grain = (ST_TARGET_GRAIN_POSITION - ST_TARGET_ATTACK) as usize;
                t.instrument[grain] = clamp(x, 0.0, 1.0);
                t.instrument_mask |= 1u64 << grain;
                t.row_locks |= 1u64 << ST_TARGET_GRAIN_POSITION;
            }
            _ => {}
        }
        if target >= 0 {
            t.row_locks |= 1u64 << target;
            let event =
                STEvent { type_: ST_EVENT_PARAMETER, track, target, value: x as f64, ..STEvent::default() };
            emit_event(&self.e.out, self.sh, event);
        }
    }

    /// Resolved locks contain only numeric dispatch data. These bounded arrays are rebuilt at
    /// row boundaries; no project identity lookup or allocation occurs on the render thread.
    /// Duplicate columns coalesce so FX2 wins before delivery.
    fn add_plugin_lock(&mut self, track: i32, fx: &PluginStepFX) {
        if !fx.present {
            return;
        }
        let count = self.e.plugin_row_lock_count;
        let locks = &mut self.tb.plugin_row_locks;
        for lock in locks.iter_mut().take(count) {
            if lock.track == track && lock.fx.slot == fx.slot && lock.fx.address == fx.address {
                lock.fx = *fx;
                lock.delivery = LaneEventState::default();
                return;
            }
        }
        if let Some(lock) = locks.get_mut(count) {
            *lock = PluginRowLock { fx: *fx, track, delivery: LaneEventState::default() };
            self.e.plugin_row_lock_count = count + 1;
        }
    }

    fn plugin_locked(&self, lane: &Lane, host: bool) -> bool {
        for lock in self.tb.plugin_row_locks.iter().take(self.e.plugin_row_lock_count) {
            if lock.fx.address != lane.address {
                continue;
            }
            let matches = if host {
                lock.fx.destination >= 0 && lock.fx.destination == lane.target
            } else {
                lane.target == ST_TARGET_PLUGIN && lock.track == lane.track && lock.fx.slot == lane.slot
            };
            if matches {
                return true;
            }
        }
        false
    }

    /// Resolve the at-most-256 lock keys against lane addresses once per row, rather than
    /// repeating the key search for every lane on every sample.
    fn index_plugin_locks(&mut self) {
        let Some(pattern) = self.p.pattern(self.e.pattern) else { return };
        for (n, lane) in pattern.lane_list().iter().enumerate().take(256) {
            self.e.plugin_lane_locked[n] = self.plugin_locked(lane, false);
        }
        for (n, lane) in pattern.host_lane_list().iter().enumerate().take(256) {
            self.e.plugin_host_lane_locked[n] = self.plugin_locked(lane, true);
        }
        self.e.plugin_locks_pending = self.e.plugin_row_lock_count as i32;
    }

    fn emit_plugin_locks(&mut self) {
        if self.e.plugin_locks_pending == 0 {
            return;
        }
        let frame = self.e.total_frames as u64;
        let host_map = self.p.host_map;
        let count = self.e.plugin_row_lock_count;
        for lock in self.tb.plugin_row_locks.iter_mut().take(count) {
            let state = &mut lock.delivery;
            if state.delivered || (state.known && frame < state.retry_frame) {
                continue;
            }
            let host = lock.fx.destination >= 0;
            let event = STEvent {
                type_: if host { ST_EVENT_HOST_PARAMETER } else { ST_EVENT_PARAMETER },
                track: if host { -1 } else { lock.track },
                target: if host { lock.fx.destination } else { ST_TARGET_PLUGIN },
                slot: lock.fx.slot,
                address: lock.fx.address,
                value: lock.fx.value,
                hostMap: if host { host_map } else { 0 },
                ..STEvent::default()
            };
            state.known = true;
            state.delivered = emit_event(&self.e.out, self.sh, event);
            state.retry_frame = frame.wrapping_add(32);
            if state.delivered {
                self.e.plugin_locks_pending -= 1;
            }
        }
    }

    /// OFF/CUT also cancel delayed triggers already queued for a lane.
    fn cancel_scheduled(&mut self, lane: i32) {
        let e = &mut *self.e;
        if e.scheduled_count == 0 {
            return;
        }
        for group in 0..cmax(e.scheduled_groups_used, 0).min(SCHEDULED_GROUPS) {
            let mut pending = e.scheduled_masks[group];
            while pending != 0 {
                let bit = lowest_bit(pending);
                pending &= pending - 1;
                let Some(q) = e.scheduled.get_mut(group * 64 + bit) else { continue };
                if q.active && q.sequence_track == lane {
                    q.active = false;
                    e.scheduled_masks[group] &= !(1u64 << bit);
                    e.scheduled_count -= 1;
                }
            }
        }
    }

    /// OFF (-1) releases and CUT (-2) kills the sequenced voices of one lane.
    fn end_lane_voices(&mut self, lane: i32, cut: bool) {
        for index in 0..MAX_VOICES {
            let v = &self.e.voices[index];
            if v.active && v.sequenced && v.sequence_track == lane {
                if cut {
                    self.kill(index);
                } else {
                    self.release(index);
                }
            }
        }
    }

    /// Extra note columns of track t on this row: each is its own lane on the track's channel.
    /// OFF/CUT end only that column's notes (and its queued hits); a note releases only that
    /// column's gateless notes (noteOn) and takes the row's note FX (ratchet, probability with
    /// one roll per row, microtiming, offset, reverse, glide, arpeggio, slice); channel FX
    /// already applied through column 0.
    fn trigger_columns(&mut self, t: i32, pattern: &Pattern, row: i32, mut fx: RowFX) {
        for c in 1..=EXTRA_COLUMNS {
            let s = pattern.column_step(t, c, row);
            if !s.present {
                continue;
            }
            let sp = &s.p;
            let lane = t + c * ST_TRACK_SANITY_LIMIT;
            if sp.note == -1 || sp.note == -2 {
                self.cancel_scheduled(lane);
                self.end_lane_voices(lane, sp.note == -2);
                continue;
            }
            if sp.note < 0 {
                continue;
            }
            if fx.roll < 0 {
                fx.roll = (fx.probability >= 1.0 || (random_bipolar(&mut self.e.rng) + 1.0) * 0.5 < fx.probability) as i32;
            }
            if fx.roll == 0 {
                continue;
            }
            if fx.ratchet <= 1 && self.consume_early_input(t, sp.note, row) {
                continue;
            }
            let instrument = if sp.instrument >= 0 { sp.instrument } else { self.track_instrument(t) };
            self.schedule_hits(&Hits {
                lane,
                channel: t,
                instrument,
                step: sp,
                count: 1,
                chord: &UNISON,
                ratchet: fx.ratchet,
                micro: fx.micro,
                offset: fx.offset,
                reverse: fx.reverse,
                glide: fx.glide,
                arp: fx.arp,
                slice: fx.slice,
            });
        }
    }

    /// `current->tracks[t].instrument`
    #[inline]
    fn track_instrument(&self, t: i32) -> i32 {
        self.p.tracks.get(t as u32 as usize).map_or(0, |track| track.instrument)
    }

    fn trigger_step(&mut self, t: i32, s: &Step, pattern: &Pattern, row: i32) {
        let columns = pattern.has_extra_columns(t);
        if !s.present {
            if columns {
                self.trigger_columns(t, pattern, row, RowFX::NONE);
            }
            return;
        }
        let sp = &s.p;
        let channel = t;
        for fx in &s.plugin_fx {
            self.add_plugin_lock(channel, fx);
        }
        if sp.note < 0 && sp.note != -3 {
            self.clear_lane_ratchets(t);
            // OFF/CUT also cancel delayed triggers already queued for this lane (column 0 only).
            self.cancel_scheduled(t);
            self.end_lane_voices(t, sp.note == -2);
            if columns {
                self.trigger_columns(t, pattern, row, RowFX::NONE);
            }
            return;
        }
        let has_ratchet = sp.fx1.type_ == ST_FX_RATCHET || sp.fx2.type_ == ST_FX_RATCHET;
        let mut has_effect = sp.fx1.type_ != ST_FX_NONE || sp.fx2.type_ != ST_FX_NONE;
        for fx in &s.plugin_fx {
            has_effect = has_effect || fx.present;
        }
        // A cell with neither note nor effect (left behind when its last effect is removed) is
        // empty: a sticky ratchet continues.
        if sp.note == -3 && !has_effect {
            if columns {
                self.trigger_columns(t, pattern, row, RowFX::NONE);
            }
            return;
        }
        // A row's own Ratchet sets the rate; a new note or another effect row plays once (the
        // sticky rate then stops). Only empty rows continue a held ratchet (see trigger_row).
        let mut fx_row = RowFX::NONE;
        let (mut has_offset, mut has_glide, mut has_arp) = (false, false, false);
        for fx in [sp.fx1, sp.fx2] {
            match fx.type_ {
                ST_FX_SLICE => fx_row.slice = clamp(finite(fx.value, 1.0), 1.0, 128.0) as i32 - 1,
                ST_FX_PROBABILITY => fx_row.probability = clamp(finite(fx.value, 0.0), 0.0, 1.0),
                ST_FX_RATCHET => fx_row.ratchet = clamp(finite(fx.value, 1.0), 1.0, 8.0) as i32,
                ST_FX_REVERSE => fx_row.reverse = (fx.value >= 0.5) as i32,
                ST_FX_OFFSET => {
                    fx_row.offset = clamp(finite(fx.value, 0.0), 0.0, 0.9999);
                    has_offset = true;
                }
                ST_FX_GLIDE => {
                    fx_row.glide = clamp(finite(fx.value, 0.0), -48.0, 48.0);
                    has_glide = true;
                }
                ST_FX_MICROTIMING => fx_row.micro = clamp(finite(fx.value, 0.0), 0.0, 0.95),
                ST_FX_ARPEGGIO => {
                    fx_row.arp = clamp(finite(fx.value, 0.0), 0.0, 255.0) as i32;
                    has_arp = true;
                }
                _ => self.apply_fx(channel, fx),
            }
        }
        let RowFX { ratchet, reverse, slice, offset, glide, arp, .. } = fx_row;
        if sp.note == -3 {
            if let Some(held) = self.tb.lane_ratchets.get_mut(t as u32 as usize) {
                held.handled = true;
                held.rate = ratchet;
            }
            // Effect-only sample commands operate on the existing playhead, preserving the
            // envelope and pitch. Offset is relative to playback direction. They steer every
            // note column's sounding voices of the track.
            for v in self.e.voices.iter_mut() {
                if !(v.active && v.sequenced && lane_track(v.sequence_track) == t && v.track == channel && !v.sample.is_null())
                {
                    continue;
                }
                let direction = if reverse != 0 { -1 } else { 1 };
                if reverse >= 0 && v.dir != direction {
                    v.sample_transition = true;
                    v.dir = direction;
                }
                if has_offset {
                    v.sample_transition = true;
                    v.pos = if v.dir > 0 {
                        v.begin + offset as f64 * (v.end - v.begin - 1.0)
                    } else {
                        v.end - 1.0 - offset as f64 * (v.end - v.begin - 1.0)
                    };
                    v.looped = false;
                    v.grain_phase = 0.0;
                }
            }
            // The same commands steer the held note's repeats, from this row on.
            let mut snapshot = None;
            if let Some(held) = self.tb.lane_ratchets.get_mut(t as u32 as usize) {
                if reverse >= 0 {
                    held.reverse = reverse;
                }
                if slice >= 0 {
                    held.slice = slice;
                }
                if has_offset {
                    held.offset = offset;
                }
                if has_glide {
                    held.glide = glide;
                }
                if has_arp {
                    held.arp = arp;
                }
                snapshot = Some(*held);
            }
            if has_ratchet {
                if let Some(held) = snapshot {
                    self.repeat_ratchet(t, channel, &held);
                }
            }
            if columns {
                self.trigger_columns(t, pattern, row, fx_row);
            }
            return;
        }
        // Roll before touching the sticky state: a dropped note leaves the row as if it held no
        // note. One roll per row: the extra note columns follow it.
        let draw = (random_bipolar(&mut self.e.rng) + 1.0) * 0.5;
        fx_row.roll = (draw < fx_row.probability || fx_row.probability >= 1.0) as i32; // the draw can round up to exactly 1
        if fx_row.roll == 0 {
            if columns {
                self.trigger_columns(t, pattern, row, fx_row);
            }
            return;
        }
        let instrument = if sp.instrument >= 0 { sp.instrument } else { self.track_instrument(t) };
        if let Some(held) = self.tb.lane_ratchets.get_mut(t as u32 as usize) {
            held.handled = true;
            held.rate = ratchet;
            if sp.velocity <= 0 {
                *held = RatchetState::default();
            } else {
                held.known = true;
                held.step = *sp;
                held.count = s.count;
                held.chord = s.chord;
                held.instrument = instrument;
                held.reverse = reverse;
                held.offset = offset;
                held.slice = slice;
                held.glide = glide;
                held.arp = arp;
            }
        }
        if !(ratchet <= 1 && s.count <= 1 && self.consume_early_input(t, sp.note, row)) {
            self.schedule_hits(&Hits {
                lane: t,
                channel,
                instrument,
                step: sp,
                count: s.count,
                chord: &s.chord,
                ratchet,
                micro: fx_row.micro,
                offset,
                reverse,
                glide,
                arp,
                slice,
            });
        }
        if columns {
            self.trigger_columns(t, pattern, row, fx_row);
        }
    }

    /// Gate is in rows of the hit spacing. No gate (<=0) is tracker semantics: the note sounds
    /// until the next note on its lane (noteOn releases it), an OFF/CUT, the end of a one-shot,
    /// or stop. Ratchet repeats of a gateless note each last one repeat spacing.
    fn schedule_hits(&mut self, hits: &Hits) {
        let sp = hits.step;
        let (ratchet, micro) = (hits.ratchet, hits.micro);
        let row_duration = self.e.row_duration;
        // `1 - micro` is formed in single precision, as in C++.
        let spacing = row_duration * (1.0 - micro) as f64 / ratchet as f64;
        let open = !(sp.gate > 0.0) && ratchet <= 1;
        let gate = if sp.gate > 0.0 { clamp(sp.gate as f64, 0.01, 256.0) } else { 1.0 };
        let mapped = self.p.timing.is_some();
        let count = clamp(hits.count, 0, ST_MAX_CHORD as i32) as usize;
        for r in 0..ratchet {
            for n in 0..count {
                let note = sp.note.wrapping_add(hits.chord[n]);
                if mapped {
                    let beat_span = self.e.row_end_beat - self.e.row_start_beat;
                    let beat_spacing = beat_span * (1.0 - micro) as f64 / ratchet as f64;
                    let onset = self.e.row_start_beat + beat_span * micro as f64 + r as f64 * beat_spacing;
                    let end = if open { -1.0 } else { onset + beat_spacing * gate };
                    self.schedule_mapped(hits.channel, hits.instrument, note, sp.velocity, onset, end, hits);
                } else {
                    let delay = row_duration * micro as f64 + r as f64 * spacing;
                    let length = if open { -1.0 } else { cmax(1.0, spacing * gate) };
                    self.schedule(hits.channel, hits.instrument, note, sp.velocity, delay, length, hits);
                }
            }
        }
    }

    fn clear_lane_ratchets(&mut self, t: i32) {
        if let Some(ratchet) = self.tb.lane_ratchets.get_mut(t as u32 as usize) {
            *ratchet = RatchetState::default();
        }
    }

    /// Replays the held note at its sticky rate with its own resolved playback values (no
    /// microtiming: repeats fill the row).
    fn repeat_ratchet(&mut self, t: i32, channel: i32, r: &RatchetState) {
        if !r.known || r.rate <= 1 {
            return;
        }
        self.schedule_hits(&Hits {
            lane: t,
            channel,
            instrument: r.instrument,
            step: &r.step,
            count: r.count,
            chord: &r.chord,
            ratchet: r.rate,
            micro: 0.0,
            offset: r.offset,
            reverse: r.reverse,
            glide: r.glide,
            arp: r.arp,
            slice: r.slice,
        });
    }

    /// Start the current row: resolve its duration, trigger every track's step (and note
    /// columns), continue sticky ratchets on empty rows.
    pub fn trigger_row(&mut self) {
        let p = self.p;
        self.e.row_has_started = true;
        if !self.e.song_mode {
            self.note_pattern_row();
        }
        self.clear_plugin_locks();
        for state in self.tb.track_state.iter_mut() {
            state.row_locks = 0;
        }
        let row = self.e.row;
        if let Some(timing) = &p.timing {
            self.e.row_start_beat = self.row_base_beat() + p.beat_for_row(row);
            self.e.row_end_beat = self.row_base_beat() + p.beat_for_row(row + 1);
            let end = timing.frame(self.e.row_end_beat) as f64;
            self.e.row_duration = end - timing.frame(self.e.row_start_beat) as f64;
            self.e.row_remaining = end - self.e.transport_frames;
        } else {
            // `sign * swing` is single precision; the sum with 1.0 is double, as in C++.
            let sign: f32 = if row & 1 != 0 { -1.0 } else { 1.0 };
            self.e.row_duration =
                self.e.sample_rate * 60.0 / p.bpm / p.lpb as f64 * (1.0 + (sign * p.swing) as f64);
            self.e.row_remaining += self.e.row_duration;
        }
        for ratchet in self.tb.lane_ratchets.iter_mut() {
            ratchet.handled = false;
        }
        if let Some(pattern) = p.pattern(self.e.pattern) {
            for t in 0..p.track_count {
                self.trigger_step(t, pattern.step(t, row), pattern, row);
                // A lane's sticky ratchets live on its own channel.
                if let Some(ratchet) = self.tb.lane_ratchets.get(t as usize).copied() {
                    if !ratchet.handled {
                        self.repeat_ratchet(t, t, &ratchet);
                    }
                }
            }
        }
        // An early note is skipped only by the very next row trigger.
        self.forget_early_inputs();
        self.index_plugin_locks();
    }

    /// Per-sample automation: deliver pending plug-in locks, evaluate native lanes into the
    /// channel state (unless the row locks them) and emit native and host lane events.
    pub fn automation(&mut self) {
        if !self.e.playing {
            return;
        }
        self.emit_plugin_locks();
        if self.e.automation_pattern != self.e.pattern {
            self.reset_automation_events();
            self.e.automation_pattern = self.e.pattern;
        }
        let p = self.p;
        let Some(pattern) = p.pattern(self.e.pattern) else { return };
        let lanes = pattern.lane_list();
        let host_lanes = pattern.host_lane_list();
        if lanes.is_empty() && host_lanes.is_empty() {
            return;
        }
        let pos = if self.e.sequence_finished { pattern.length as f64 } else { self.musical_position() };
        let row_base_beat = self.row_base_beat();
        let musical_beat = self.e.musical_beat;
        let frame = self.e.total_frames as u64;
        let track_count = p.track_count;
        let e = &mut *self.e;
        for (index, l) in lanes.iter().enumerate().take(256) {
            let state = &mut e.lane_event_state[index];
            if !(l.track >= 0 && l.track < track_count) || l.points.is_empty() {
                continue;
            }
            let v = lane_value(p, musical_beat, row_base_beat, l, pos) as f32;
            let Some(t) = self.tb.track_state.get_mut(l.track as usize) else { continue };
            if (t.row_locks & (1u64 << (l.target as u32 & 63))) != 0 || e.plugin_lane_locked[index] {
                state.known = false;
                state.delivered = false;
                continue;
            }
            match l.target {
                ST_TARGET_VOLUME => t.volume = clamp(v, 0.0, 2.0),
                ST_TARGET_PAN => t.pan = clamp(v, -1.0, 1.0),
                ST_TARGET_CUTOFF => t.cutoff = clamp(v, 20.0, 22000.0),
                ST_TARGET_RESONANCE => t.resonance = clamp(v, 0.0, 0.99),
                ST_TARGET_DELAY => t.delay = clamp(v, 0.0, 1.0),
                ST_TARGET_REVERB => t.reverb = clamp(v, 0.0, 1.0),
                ST_TARGET_PITCH => t.pitch = clamp(v, -48.0, 48.0),
                ST_TARGET_PLUGIN => {}
                target => {
                    if (ST_TARGET_ATTACK..=ST_TARGET_LAST_INSTRUMENT).contains(&target) {
                        let slot = (target - ST_TARGET_ATTACK) as usize;
                        t.instrument[slot] = v;
                        t.instrument_mask |= 1u64 << slot;
                    }
                }
            }
            let event_value = if l.target == ST_TARGET_PLUGIN { clamp(v as f64, 0.0, 1.0) } else { v as f64 };
            emit_lane_automation(&e.out, self.sh, frame, p.host_map, l, state, event_value, ST_EVENT_PARAMETER);
        }
        for (index, l) in host_lanes.iter().enumerate().take(256) {
            let state = &mut e.host_lane_event_state[index];
            if e.plugin_host_lane_locked[index] {
                state.known = false;
                state.delivered = false;
                continue;
            }
            if !l.points.is_empty() {
                let value = lane_value(p, musical_beat, row_base_beat, l, pos);
                emit_lane_automation(&e.out, self.sh, frame, p.host_map, l, state, value, ST_EVENT_HOST_PARAMETER);
            }
        }
    }

    /// Start scheduled hits that are due at this sample.
    pub fn start_due_hits(&mut self) {
        if self.e.scheduled_count == 0 {
            return;
        }
        let track_count = self.p.track_count;
        let groups = self.e.scheduled_groups_used.min(SCHEDULED_GROUPS);
        for group in 0..groups {
            let mut active = self.e.scheduled_masks[group];
            while active != 0 {
                let bit = lowest_bit(active);
                active &= active - 1;
                let Some(&s) = self.e.scheduled.get(group * 64 + bit) else { continue };
                let due = if s.mapped {
                    self.e.musical_beat + self.e.loop_beat_offset >= s.onset_beat
                } else {
                    s.due <= self.e.total_frames
                };
                if !due {
                    continue;
                }
                if lane_track(s.sequence_track) < track_count {
                    self.note_on(NoteOn {
                        track: s.track,
                        instrument: s.instrument,
                        note: s.note,
                        velocity: s.velocity,
                        gate: if s.mapped { -1.0 } else { s.gate },
                        offset: s.offset,
                        reverse: s.reverse,
                        glide: s.glide,
                        arp: s.arp,
                        from_sequencer: true,
                        slice: s.slice,
                        preview: false,
                        preview_pitch: 0.0,
                        mapped_end: if s.mapped && !s.open { s.end_beat } else { -1.0 },
                        sequence_track: s.sequence_track,
                    });
                }
                if let Some(slot) = self.e.scheduled.get_mut(group * 64 + bit) {
                    slot.active = false;
                }
                self.e.scheduled_masks[group] &= !(1u64 << bit);
                self.e.scheduled_count -= 1;
            }
        }
    }
}

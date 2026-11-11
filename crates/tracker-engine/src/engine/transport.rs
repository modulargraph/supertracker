//! Transport and command handling: project adoption and the command queue (`refresh`),
//! Play/Stop/Seek/count-in, the musical clock (`update_clock`), timeline resolution, row advance
//! with song/pattern loops and queued jumps, input acknowledgements, status publication and the
//! quiescent flush used by transport checkpoints.
//!
//! The transport counts frames at the project's one tempo: a row lasts
//! `rate * 60 / bpm / lines per beat` frames (swing lengthens even rows and shortens odd ones),
//! and beats, bars and seconds are derived from the frame count.

use super::abi::*;
use super::project::Project;
use super::state::*;
use super::tables::LaneEventState;
use super::util::{clamp, cmax, cmin, finite, finite64, next_down};
use super::voice::{NoteOn, VoiceSink};
use std::sync::atomic::{fence, Ordering};

impl Render {
    pub fn reset_input_run(&mut self) {
        if self.input_run_id != 0 {
            self.input_run_id = if self.input_run_id == u64::MAX { 0 } else { self.input_run_id + 1 };
        }
        self.input_loop_beats = 0.0;
    }

    /// `clearEffects`: silence the send returns (delay, pre-delay, reverb).
    pub fn clear_effects(&mut self) {
        self.delay_tone_l = 0.0;
        self.delay_tone_r = 0.0;
        self.pre_l.fill(0.0);
        self.pre_r.fill(0.0);
        self.pre_pos = 0;
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        for comb in self.comb_l.iter_mut().chain(self.comb_r.iter_mut()) {
            comb.clear();
        }
        for allpass in self.ap_l.iter_mut().chain(self.ap_r.iter_mut()) {
            allpass.clear();
        }
    }

    /// Whether `sh` is the shared state of this render state's own engine (see
    /// `Control::owns`); the render entry points do nothing with another engine's.
    #[inline]
    pub(crate) fn owns(&self, sh: &Shared) -> bool {
        sh.engine_id == self.engine_id
    }

    /// Direct stereo render of the engine's own mix: `frames` frames into whichever of the two
    /// buffers are given (a buffer shorter than `frames` receives what fits). Without buffers
    /// the engine still advances, so `render(sh, None, None, 0)` applies queued commands and a
    /// new project.
    pub fn render(&mut self, sh: &Shared, mut left: Option<&mut [f32]>, mut right: Option<&mut [f32]>, frames: u32) {
        if !self.owns(sh) {
            for out in [left.as_deref_mut(), right.as_deref_mut()].into_iter().flatten() {
                out.fill(0.0);
            }
            return;
        }
        self.process(sh, left, right, frames, false, 0);
    }

    /// Exclusive stopped-graph operation.
    pub fn capture_transport(&mut self, sh: &Shared) -> STTransportState {
        if !self.owns(sh) {
            return STTransportState::default();
        }
        self.flush_quiescent(sh);
        // SAFETY: `&mut self` is the render half and no adoption happens while the view exists.
        unsafe { Rt::attach(self, sh) }.capture_transport()
    }

    /// Exclusive stopped-graph operation.
    pub fn restore_transport(&mut self, sh: &Shared, state: &STTransportState, resume: bool) {
        if !self.owns(sh) {
            return;
        }
        self.flush_quiescent(sh);
        // SAFETY: as in `capture_transport`.
        unsafe { Rt::attach(self, sh) }.restore_transport(state, resume);
    }

    /// End every AU/external-MIDI voice on `channel`
    /// without emitting its note-off. Returns the number of voices ended.
    pub fn forget_external_voices(&mut self, sh: &Shared, channel: i32) -> i32 {
        if !self.owns(sh) {
            return 0;
        }
        // SAFETY: `tb` points to the tables of the project in use; `&mut self` is the render
        // half, their only accessor.
        let channels = unsafe { &*self.tb }.channels;
        if channel < 0 || channel >= channels {
            return 0;
        }
        let mut count = 0;
        let mut sink = VoiceSink { masks: &mut self.active_voice_masks, out: &self.out, sh };
        for (index, v) in self.voices.iter_mut().enumerate() {
            if v.active && v.external && v.track == channel {
                v.off_sent = true;
                sink.kill(v, index);
                count += 1;
            }
        }
        count
    }

    /// `refresh`: adopt the latest published project, then apply queued commands and a pending
    /// automation refresh. Called at the start of every newly rendered block.
    pub(crate) fn refresh(&mut self, sh: &Shared) {
        let next = sh.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !next.is_null() {
            self.adopt_project(sh, next);
        }
        // SAFETY: called on the render thread (or by an exclusive stopped-graph operation); the
        // view ends with this function, before any later adoption.
        let mut rt = unsafe { Rt::attach(self, sh) };
        rt.run_commands();
        // This request has a separate mailbox: AU graph preparation may call it on the audio
        // thread while the single UI producer writes normal commands.
        if sh.automation_refresh_requested.swap(false, Ordering::AcqRel) {
            rt.emit_mix_state();
            rt.reset_automation_events();
            rt.e.plugin_locks_pending = rt.e.plugin_row_lock_count as i32;
            let count = rt.e.plugin_row_lock_count;
            for lock in rt.tb.plugin_row_locks.iter_mut().take(count) {
                lock.delivery = LaneEventState::default();
            }
        }
    }

    /// Render thread, at a project swap whose tables differ: carry sticky ratchets, pitch bend
    /// and mod wheel over by track. Voices of removed tracks end. No allocation: both tables are
    /// prebuilt.
    fn migrate_tables(&mut self, sh: &Shared, next: *mut super::tables::TablesState) {
        // SAFETY: `self.tb` and `next` are distinct tables (the caller checked), each kept alive
        // by a project this thread holds (`current` and the project being adopted), and only the
        // render thread touches table state.
        let (from, to) = unsafe { (&*self.tb, &mut *next) };
        let shared_tracks = cmin(from.track_count, to.track_count) as usize;
        for track in 0..shared_tracks {
            if let (Some(a), Some(b)) = (to.lane_ratchets.get_mut(track), from.lane_ratchets.get(track)) {
                *a = *b;
            }
            if let (Some(a), Some(b)) = (to.pitch_bend.get_mut(track), from.pitch_bend.get(track)) {
                *a = *b;
            }
            if let (Some(a), Some(b)) = (to.mod_wheel.get_mut(track), from.mod_wheel.get(track)) {
                *a = *b;
            }
        }
        let track_count = to.track_count;
        let mut sink = VoiceSink { masks: &mut self.active_voice_masks, out: &self.out, sh };
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if voice.active && voice.track >= track_count {
                sink.kill(voice, index);
            }
        }
    }

    /// The first half of C++ `refresh()`: swap in a newly published project, keeping the musical
    /// position, and retire the previous one for `collect()`.
    fn adopt_project(&mut self, sh: &Shared, next_ptr: *mut Project) {
        let old_ptr = self.current;
        // SAFETY: `old_ptr` is the project in use: alive until this function pushes it onto the
        // retired list below, after which `old` is not used again. `next_ptr` was just taken
        // from `pending`, so this thread owns it; published projects are immutable.
        let (old, next): (&Project, &Project) = unsafe { (&*old_ptr, &*next_ptr) };
        let rate = self.sample_rate;
        // Transport frames are bpm-relative: rescale them so a tempo change keeps the musical
        // position (Song mode re-resolves its row from these frames) and the current row ends
        // at the new tempo.
        if old.bpm != next.bpm {
            let scale = old.bpm / next.bpm;
            self.transport_frames *= scale;
            if self.row_has_started && self.row_remaining < 1e99 {
                self.row_remaining *= scale;
                self.row_duration *= scale;
            }
            // The row's pending hits (ratchet repeats, microtiming) and sequenced gates are
            // frame counts too: keep their musical positions.
            let total = self.total_frames;
            for s in self.scheduled.iter_mut() {
                if s.active {
                    s.due = total + (s.due - total) * scale;
                    if s.gate > 0.0 {
                        s.gate = cmax(1.0, s.gate * scale);
                    }
                }
            }
            for v in self.voices.iter_mut() {
                if v.active && v.sequenced && v.gate > 0.0 {
                    v.gate *= scale;
                }
            }
        }
        let mut arrangement_changed = old.sequence != next.sequence || old.lpb != next.lpb || old.swing != next.swing;
        if !arrangement_changed {
            for &index in &next.sequence {
                if index >= old.patterns.len() as i32 || old.pattern_length(index) != next.pattern_length(index) {
                    arrangement_changed = true;
                    break;
                }
            }
        }
        let timeline_changed = arrangement_changed || old.bpm != next.bpm;
        let next_song_end_frames = next.effective_song_frames(rate);
        let input_pattern_changed = self.pattern >= next.patterns.len() as i32
            || old.pattern_length(self.pattern) != next.pattern_length(self.pattern);
        let input_timeline_changed = arrangement_changed
            || input_pattern_changed
            || self.song_end_frames != next_song_end_frames
            || old.bpm != next.bpm
            || old.loop_song != next.loop_song;
        if input_timeline_changed {
            self.reset_input_run();
        }
        // Mixer default refreshes and pattern transitions must not erase tracker repeat memory;
        // tables only change with the track count.
        let next_tb = match &next.tables {
            Some(tables) => tables.state_ptr(),
            None => self.tb, // Never published without tables (commit builds them).
        };
        if next_tb != self.tb {
            self.migrate_tables(sh, next_tb);
        }
        self.current = next_ptr;
        self.tb = next_tb;
        if old.host_mixing != next.host_mixing {
            self.clear_effects();
        }
        self.song_end_frames = next_song_end_frames;
        self.pattern = clamp(self.pattern, 0, next.patterns.len() as i32 - 1);
        self.row = clamp(self.row, 0, next.pattern_length(self.pattern) - 1);
        self.sequence = clamp(self.sequence, 0, next.sequence.len() as i32 - 1);
        {
            // SAFETY: render thread; `current` is now the adopted project, and this view ends
            // before the next adoption.
            let mut rt = unsafe { Rt::attach(self, sh) };
            // A playing song finds its row again when the timeline changed under it, or when
            // the sequence had ended (a longer arrangement or a clip tail may continue it).
            if rt.e.playing && rt.e.song_mode && (timeline_changed || rt.e.sequence_finished) {
                rt.resolve_timeline();
            }
        }
        // Retire the previous project on the lock-free list; `old` must not be used from here on.
        let mut head = sh.retired.load(Ordering::Relaxed);
        loop {
            // SAFETY: `old_ptr` is still exclusively the render thread's until the exchange below
            // succeeds; `retired_next` is only read by `collect()` after it takes the list.
            unsafe { (*old_ptr).retired_next = head };
            match sh.retired.compare_exchange_weak(head, old_ptr, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => break,
                Err(actual) => head = actual,
            }
        }
        // SAFETY: as above.
        let mut rt = unsafe { Rt::attach(self, sh) };
        rt.e.live_param_count = 0;
        rt.reset_track_state(true);
        rt.reconcile_voices();
        sh.status_tracks.store(rt.p.track_count, Ordering::Relaxed);
    }

    /// Adopt pending project/control changes at zero audio time (transport capture/restore).
    /// Discards queued events.
    pub(crate) fn flush_quiescent(&mut self, sh: &Shared) {
        self.out.suppress_events = true;
        self.out.frame_offset = 0;
        self.refresh(sh);
        {
            // SAFETY: exclusive stopped-graph operation standing in for the render thread; the
            // view ends before any later adoption.
            let mut rt = unsafe { Rt::attach(self, sh) };
            rt.finish_input_acks(false, false, 0);
            while sh.events.pop().is_some() {}
            rt.publish_transport();
        }
        self.out.suppress_events = false;
        sh.automation_refresh_requested.store(true, Ordering::Release);
    }
}

impl<'a> Rt<'a> {
    /// After a project swap: voices of disabled channels and removed instruments end, native
    /// voices pick up the committed instrument settings, external voices that may no longer
    /// sound are released.
    fn reconcile_voices(&mut self) {
        let p = self.p;
        for index in 0..MAX_VOICES {
            let v = &self.e.voices[index];
            if v.active && !self.channel_enabled(v.track) {
                self.kill(index);
            }
        }
        for index in 0..MAX_VOICES {
            let v = &self.e.voices[index];
            if v.active && !v.external && !p.has_instrument(v.instrument) {
                self.kill(index);
            }
        }
        for v in self.e.voices.iter_mut() {
            if !(v.active && !v.external) {
                continue;
            }
            let Some(instrument) = p.instruments.get(v.instrument as usize) else { continue };
            let i = &instrument.p;
            if i.type_ != v.p.type_ {
                continue;
            }
            v.p.volume = i.volume * v.zone_volume;
            v.p.pan = clamp(i.pan + v.zone_pan, -1.0, 1.0);
            v.p.tune = i.tune + v.zone_tune;
            v.p.root_note = if v.zone_root >= 0 { v.zone_root } else { i.root_note };
            v.p.attack = i.attack;
            v.p.decay = i.decay;
            v.p.sustain = i.sustain;
            v.p.release = i.release;
            v.p.cutoff = i.cutoff;
            v.p.resonance = i.resonance;
            v.p.drive = i.drive;
            v.p.bit_depth = i.bit_depth;
            v.p.lfo_rate = i.lfo_rate;
            v.p.lfo_depth = i.lfo_depth;
            v.p.lfo_target = i.lfo_target;
            v.p.lfo_shape = i.lfo_shape;
            v.p.lfo2_rate = i.lfo2_rate;
            v.p.lfo2_depth = i.lfo2_depth;
            v.p.lfo2_target = i.lfo2_target;
            v.p.lfo2_shape = i.lfo2_shape;
            v.p.lfo_sync = i.lfo_sync;
            v.p.lfo2_sync = i.lfo2_sync;
            v.p.lfo_beats = i.lfo_beats;
            v.p.lfo2_beats = i.lfo2_beats;
            v.p.filter_attack = i.filter_attack;
            v.p.filter_decay = i.filter_decay;
            v.p.filter_sustain = i.filter_sustain;
            v.p.filter_release = i.filter_release;
            v.p.filter_env_amount = i.filter_env_amount;
            v.p.grain_size = i.grain_size;
            v.p.grain_position = i.grain_position;
            v.p.wavetable_position = i.wavetable_position;
            v.p.grain_density = i.grain_density;
            v.p.grain_spray = i.grain_spray;
            v.p.grain_pitch_spread = i.grain_pitch_spread;
            v.p.grain_scan = i.grain_scan;
            v.p.grain_reverse = i.grain_reverse;
            v.p.grain_stereo = i.grain_stereo;
        }
        for index in 0..MAX_VOICES {
            let v = &self.e.voices[index];
            if !(v.active && v.external) {
                continue;
            }
            let track = v.track;
            let kind = match p.instruments.get(v.instrument as u32 as usize) {
                Some(instrument) => instrument.p.type_,
                None => ST_INSTRUMENT_SAMPLE,
            };
            if !self.channel_enabled(track)
                || !self.external_track_enabled(track, kind)
                || (kind != ST_INSTRUMENT_AU && kind != ST_INSTRUMENT_MIDI)
            {
                self.release(index);
            }
        }
    }

    /// The second half of C++ `refresh()`: drain the command ring.
    ///
    /// A concurrent producer may refill the command ring while it is drained; the batch is
    /// bounded by the input acknowledgements one block can hold.
    fn run_commands(&mut self) {
        while self.e.pending_input_count < MAX_COMMANDS {
            let Some(c) = self.sh.commands.pop() else { break };
            match c.kind {
                CMD_PLAY => {
                    self.e.count_in_total = 0;
                    self.start_playback(c.a != 0, false);
                    self.applied_transport(&c);
                }
                CMD_STOP => {
                    self.clear_ratchets();
                    self.applied_transport(&c);
                    self.clear_queued();
                    self.e.count_in_total = 0;
                    self.reset_pattern_clips();
                    self.e.reset_input_run();
                    self.e.playing = false;
                    self.clear_plugin_locks();
                    self.reset_automation_events();
                    self.all_off(false, false);
                    self.clear_scheduled();
                }
                CMD_SEEK => {
                    self.e.seek_revision = self.e.seek_revision.wrapping_add(1);
                    self.clear_ratchets();
                    self.forget_early_inputs();
                    self.clear_queued();
                    self.reset_pattern_clips();
                    self.e.reset_input_run();
                    self.all_off(true, false);
                    self.e.pattern = self.valid_pattern(c.a);
                    self.e.row = clamp(c.b, 0, self.p.pattern_length(self.e.pattern) - 1);
                    self.e.sequence = clamp(c.c, 0, self.p.sequence.len() as i32 - 1);
                    self.e.row_remaining = 0.0;
                    self.e.row_has_started = false;
                    self.e.sequence_finished = false;
                    self.reset_track_state(true);
                    self.e.transport_frames = self.seek_frames();
                    self.e.click_beat = -1;
                    self.clear_scheduled();
                }
                CMD_NOTE_ON => self.note_on(NoteOn::live(c.a, c.b, c.c, c.d)),
                CMD_NOTE_OFF => {
                    for index in 0..MAX_VOICES {
                        let v = &self.e.voices[index];
                        if v.active && !v.sequenced && !v.preview && v.track == c.a && (c.b < 0 || v.note == c.b) {
                            self.release(index);
                        }
                    }
                }
                CMD_PANIC => {
                    self.clear_ratchets();
                    self.forget_early_inputs();
                    self.reset_pattern_clips();
                    self.e.reset_input_run();
                    self.clear_plugin_locks();
                    self.reset_automation_events();
                    self.all_off(true, false);
                    self.clear_scheduled();
                    self.e.clear_effects();
                    self.tb.pitch_bend.fill(0.0);
                    self.tb.mod_wheel.fill(0.0);
                    self.clear_queued();
                    self.e.count_in_total = 0;
                    self.e.click_age = 1e9;
                }
                CMD_PITCH_BEND => {
                    if c.a >= 0 && c.a < self.tb.channels {
                        if let Some(bend) = self.tb.pitch_bend.get_mut(c.a as usize) {
                            *bend = clamp(finite(c.value, 0.0), -48.0, 48.0);
                        }
                    }
                }
                CMD_PREVIEW => self.preview_step(&c),
                CMD_QUEUE_JUMP => self.queue_jump(c.a, c.b),
                CMD_QUEUE_MODE => self.queue_mode_switch(c.a != 0, c.b, c.c),
                CMD_COUNT_IN => {
                    self.begin_count_in(c.a != 0, c.b);
                    self.applied_transport(&c);
                }
                CMD_MOD_WHEEL => {
                    if c.a >= 0 && c.a < self.tb.channels {
                        if let Some(wheel) = self.tb.mod_wheel.get_mut(c.a as usize) {
                            *wheel = clamp(finite(c.value, 0.0), 0.0, 1.0);
                        }
                    }
                }
                CMD_LIVE_PARAM => self.set_live_param(c.a, c.b, c.value),
                CMD_RECORDING => self.e.recording = c.a != 0,
                CMD_PATTERN_LOOP => {
                    let (mut start, mut end) = (c.a, c.b);
                    if start < 0 || end < start {
                        start = -1;
                        end = -1;
                    }
                    if start == self.e.loop_start_row && end == self.e.loop_end_row {
                        continue;
                    }
                    self.e.loop_start_row = start;
                    self.e.loop_end_row = end;
                    if self.e.song_mode {
                        continue;
                    }
                    self.e.reset_input_run();
                }
                CMD_INPUT_NOTE => self.input_note_command(&c),
                _ => {}
            }
        }
    }

    /// Command 10: an input edge that is acknowledged with its applied clock position.
    fn input_note_command(&mut self, c: &Command) {
        if c.apply_input_note != 0 {
            if c.input_edge == ST_INPUT_NOTE_ON {
                self.note_on(NoteOn::live(c.a, c.b, c.c, c.d));
            } else {
                for index in 0..MAX_VOICES {
                    let v = &self.e.voices[index];
                    if v.active && !v.sequenced && !v.preview && v.track == c.a && v.note == c.c {
                        self.release(index);
                    }
                }
            }
        }
        let e = &mut *self.e;
        let Some(ack) = e.pending_input_acks.get_mut(e.pending_input_count) else { return };
        e.pending_input_count += 1;
        *ack = STInputAck::default();
        ack.request_id = c.request_id;
        ack.run_id = e.input_run_id;
        ack.edge = c.input_edge;
        ack.track = c.a;
        ack.instrument = c.b;
        ack.note = c.c;
        ack.velocity = c.d;
        ack.apply_note = c.apply_input_note;
        ack.early = 0;
        if c.input_edge == ST_INPUT_NOTE_ON
            && e.playing
            && e.row_has_started
            && e.row_duration > 0.0
            && e.row_remaining < e.row_duration * 0.5
        {
            let length = self.p.pattern_length(e.pattern);
            if e.row + 1 < length && !(e.loop_end_row >= 0 && e.row == e.loop_end_row) {
                ack.early = 1;
                ack.pattern = e.pattern;
                ack.row = e.row + 1;
                ack.sequence = e.sequence;
                e.early_inputs[e.early_input_next % 16] =
                    EarlyInput { track: c.a, note: c.c, pattern: e.pattern, row: e.row + 1 };
                e.early_input_next = (e.early_input_next + 1) % 16;
            }
        }
    }

    fn clear_queued(&mut self) {
        self.e.queued_pattern = -1;
        self.e.queued_sequence = -1;
        self.e.queued_mode = -1;
    }

    fn applied_transport(&mut self, c: &Command) {
        if c.d != 0 {
            self.e.transport_applied = c.d as u32;
        }
    }

    /// `current->sequence[index]` (the sequence is never empty and `index` is kept in range).
    #[inline]
    pub fn sequence_pattern(&self, index: i32) -> i32 {
        self.p.sequence.get(index as u32 as usize).copied().unwrap_or(0)
    }

    pub fn finish_input_acks(&mut self, audio_frame: bool, in_block: bool, graph_frame: u64) {
        if audio_frame {
            self.update_clock();
        }
        let e = &mut *self.e;
        let count = cmin(e.pending_input_count, e.pending_input_acks.len());
        for n in 0..count {
            let mut ack = e.pending_input_acks[n];
            ack.render_frame = e.total_frames as i64;
            ack.graph_frame = if in_block && graph_frame <= (1u64 << 52) { graph_frame as i64 } else { i64::MIN };
            // An early note keeps its placed row while that row is the one now playing or still
            // ahead.
            if ack.early == 0 || ack.pattern != e.pattern || ack.sequence != e.sequence || e.row > ack.row {
                ack.pattern = e.pattern;
                ack.row = e.row;
                ack.sequence = e.sequence;
                ack.early = 0;
            }
            ack.status = if !audio_frame {
                ST_INPUT_ACK_CONTROL_FLUSH
            } else if e.input_run_id == 0 || ack.run_id != e.input_run_id {
                ST_INPUT_ACK_DISCONTINUITY
            } else {
                ST_INPUT_ACK_APPLIED
            };
            if ack.status == ST_INPUT_ACK_APPLIED {
                ack.distance_beats = e.musical_beat + e.input_loop_beats;
                ack.flags = (if e.playing { ST_INPUT_CLOCK_PLAYING } else { 0 })
                    | (if e.recording { ST_INPUT_CLOCK_RECORDING } else { 0 });
                if !ack.distance_beats.is_finite() || ack.distance_beats < 0.0 {
                    ack.status = ST_INPUT_ACK_DISCONTINUITY;
                    ack.flags = 0;
                    ack.distance_beats = 0.0;
                }
            }
            // Admission reserves a slot for every accepted command, including
            // queued-but-not-yet-rendered edges; the ACK ring cannot overflow.
            let pushed = self.sh.input_acks.push(&ack);
            debug_assert!(pushed);
        }
        e.pending_input_count = 0;
    }

    /// Play. A count-in hands over with `keep_live_voices` so notes played during the pre-roll
    /// keep sounding.
    pub fn start_playback(&mut self, song: bool, keep_live_voices: bool) {
        let p = self.p;
        self.clear_ratchets();
        self.reset_pattern_clips();
        self.e.reset_input_run();
        self.all_off(true, keep_live_voices);
        self.e.playing = true;
        self.e.song_mode = song;
        if song {
            self.e.sequence = clamp(self.e.sequence, 0, p.sequence.len() as i32 - 1);
            self.e.pattern = self.valid_pattern(self.sequence_pattern(self.e.sequence));
        } else {
            self.e.pattern = self.valid_pattern(p.active_pattern);
        }
        self.e.row = clamp(self.e.row, 0, p.pattern_length(self.e.pattern) - 1);
        if self.pattern_loop_active() && (self.e.row < self.e.loop_start_row || self.e.row > self.pattern_loop_end()) {
            self.e.row = self.e.loop_start_row;
        }
        self.e.row_remaining = 0.0;
        self.e.row_has_started = false;
        self.e.sequence_finished = false;
        self.reset_track_state(true);
        self.e.transport_frames = self.seek_frames();
        self.e.click_beat = -1;
        self.clear_scheduled();
        self.clear_queued();
    }

    /// Count-in: one click per beat of the project's meter, then Play at the exact frame.
    fn begin_count_in(&mut self, song: bool, bars: i32) {
        if self.e.playing {
            return;
        }
        let p = self.p;
        let e = &mut *self.e;
        let click = (e.sample_rate * 60.0 / cmax(1.0, p.bpm) * 4.0 / cmax(1, p.denominator) as f64).round() as i64;
        e.count_in_click = cmax(1, click);
        e.count_in_bar = cmax(1, p.numerator) as i64;
        e.count_in_total = e.count_in_click.wrapping_mul(e.count_in_bar).wrapping_mul(clamp(bars, 1, 4) as i64);
        e.count_in_elapsed = 0;
        e.count_in_song = song;
        e.click_age = 1e9;
    }

    pub fn step_count_in(&mut self) {
        if self.e.count_in_elapsed >= self.e.count_in_total {
            self.e.count_in_total = 0;
            self.start_playback(self.e.count_in_song, true);
            return;
        }
        let e = &mut *self.e;
        let click = cmax(1, e.count_in_click);
        if e.count_in_elapsed % click == 0 {
            e.click_age = 0.0;
            e.click_phase = 0.0;
            e.click_accent = (e.count_in_elapsed / click) % cmax(1, e.count_in_bar) == 0;
        }
        e.count_in_elapsed += 1;
    }

    pub fn song_loop_active(&self) -> bool {
        let p = self.p;
        self.e.song_mode
            && p.loop_song
            && p.song_loop_start >= 0
            && p.song_loop_end >= p.song_loop_start
            && p.song_loop_start < p.sequence.len() as i32
    }

    pub fn song_loop_last(&self) -> i32 {
        cmin(self.p.song_loop_end, self.p.sequence.len() as i32 - 1)
    }

    /// The quarter beat at which sequence entry `index` starts.
    pub fn occurrence_beat(&self, index: i32) -> f64 {
        let p = self.p;
        let mut rows = 0.0f64;
        let mut n = 0;
        while n < index && n < p.sequence.len() as i32 {
            rows += p.pattern_rows(self.sequence_pattern(n));
            n += 1;
        }
        rows / p.lpb as f64
    }

    pub fn occurrence_start_frames(&self, index: i32) -> f64 {
        self.occurrence_beat(index) * self.e.sample_rate * 60.0 / self.p.bpm
    }

    /// Continue at the section start. From a row boundary the fractional overshoot is kept;
    /// `exact` (song end with clip tails) restarts the section at its first frame.
    fn loop_song_section(&mut self, from_index: i32, exact: bool, target: i32) {
        let start = if target >= 0 { target } else { self.p.song_loop_start };
        let beats = self.occurrence_beat(from_index) - self.occurrence_beat(start);
        self.e.input_loop_beats += beats;
        self.e.click_beat = -1;
        if exact {
            self.e.transport_frames = self.occurrence_start_frames(start);
            self.e.row_remaining = 0.0;
            self.e.row_has_started = false;
            self.all_off(false, true);
        } else {
            self.e.transport_frames -= self.occurrence_start_frames(from_index) - self.occurrence_start_frames(start);
        }
        self.e.sequence = start;
        self.e.pattern = self.valid_pattern(self.sequence_pattern(start));
        self.e.row = 0;
        self.e.sequence_finished = false;
        self.reset_track_state(true);
    }

    pub fn pattern_loop_active(&self) -> bool {
        let e = &*self.e;
        !e.song_mode
            && e.loop_start_row >= 0
            && e.loop_end_row >= e.loop_start_row
            && e.loop_start_row < self.p.pattern_length(e.pattern)
    }

    pub fn pattern_loop_end(&self) -> i32 {
        cmin(self.e.loop_end_row, self.p.pattern_length(self.e.pattern) - 1)
    }

    /// The position in the pattern, in rows, with the fraction of the current row played.
    pub fn musical_position(&self) -> f64 {
        let e = &*self.e;
        e.row as f64 + clamp(1.0 - e.row_remaining / e.row_duration, 0.0, 1.0)
    }

    /// The musical clock of the transport frame: the beat, the bar number and the beat at which
    /// that bar starts, from the project's tempo and meter.
    ///
    /// Seeks keep fractional frames and the tempo goes up to 999 BPM, so the bar is found from
    /// the exact frame (including negative startup time), not from the rounded beat alone.
    pub fn update_clock(&mut self) {
        let p = self.p;
        let e = &mut *self.e;
        let (frame, rate, bpm) = (e.transport_frames, e.sample_rate, p.bpm);
        let mut beat = frame * bpm / (60.0 * rate);
        let width = p.numerator as f64 * 4.0 / p.denominator as f64;
        let mut bar = (beat / width).floor() as i64;
        // Frames from the transport frame to the first sample not earlier than `beat`.
        let offset = |beat: f64| -> i64 { (beat * (60.0 * rate) / bpm - frame).ceil() as i64 };
        for _ in 0..2 {
            if offset(bar as f64 * width) > 0 {
                bar = bar.wrapping_sub(1);
            } else if offset(bar.wrapping_add(1) as f64 * width) <= 0 {
                bar = bar.wrapping_add(1);
            } else {
                break;
            }
        }
        let bar_start = bar as f64 * width;
        let bar_end = bar.wrapping_add(1) as f64 * width;
        beat = cmax(beat, bar_start);
        if beat >= bar_end {
            beat = next_down(bar_end);
        }
        e.musical_beat = beat;
        e.bar = bar.wrapping_add(1);
        e.bar_start_beat = bar_start;
    }

    /// Publish the transport status snapshot (seqlock on `status_publication`).
    pub fn publish_transport(&mut self) {
        let sh = self.sh;
        sh.status_publication.fetch_add(1, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        self.update_clock();
        let e = &*self.e;
        let relaxed = Ordering::Relaxed;
        sh.status_seek_revision.store(e.seek_revision, relaxed);
        sh.status_beat.store(e.musical_beat, relaxed);
        sh.status_bpm.store(self.p.bpm, relaxed);
        sh.status_bar.store(e.bar, relaxed);
        sh.status_bar_start.store(e.bar_start_beat, relaxed);
        sh.status_numerator.store(self.p.numerator, relaxed);
        sh.status_denominator.store(self.p.denominator, relaxed);
        sh.status_playing.store(e.playing as i32, relaxed);
        sh.status_recording.store(e.recording as i32, relaxed);
        sh.status_song_mode.store(e.song_mode as i32, relaxed);
        sh.status_pattern.store(e.pattern, relaxed);
        sh.status_row.store(e.row, relaxed);
        sh.status_sequence.store(e.sequence, relaxed);
        sh.status_queued_pattern.store(e.queued_pattern, relaxed);
        sh.status_queued_sequence.store(e.queued_sequence, relaxed);
        let count_in = if e.count_in_total > 0 {
            let click = cmax(1, e.count_in_click);
            cmax(1, (e.count_in_total - e.count_in_elapsed + click - 1) / click) as i32
        } else {
            0
        };
        sh.status_count_in.store(count_in, relaxed);
        sh.status_position.store(if e.row_has_started { self.musical_position() } else { e.row as f64 }, relaxed);
        sh.status_seconds.store(e.transport_frames / e.sample_rate, relaxed);
        sh.status_transport_applied.store(e.transport_applied, Ordering::Release);
        sh.status_publication.fetch_add(1, Ordering::SeqCst);
    }

    /// Song mode: find the sequence position and row of `transport_frames`.
    pub fn resolve_timeline(&mut self) {
        let p = self.p;
        let mut cursor = 0.0f64;
        let base = self.e.sample_rate * 60.0 / p.bpm / p.lpb as f64;
        for s in 0..p.sequence.len() {
            let pattern = p.sequence[s];
            let duration = p.pattern_rows(pattern) * base;
            if self.e.transport_frames < cursor + duration - 1e-7 {
                self.e.sequence = s as i32;
                self.e.pattern = pattern;
                let mut local = self.e.transport_frames - cursor;
                for r in 0..p.pattern_length(pattern) {
                    // `1 + sign * swing` is formed in single precision, as in C++.
                    let length = base * (1.0f32 + (if r & 1 != 0 { -1.0f32 } else { 1.0f32 }) * p.swing) as f64;
                    if local < length - 1e-7 {
                        self.e.row = r;
                        self.e.row_duration = length;
                        self.e.row_remaining = length - local;
                        self.e.row_has_started = true;
                        self.e.sequence_finished = false;
                        self.clear_scheduled();
                        return;
                    }
                    local -= length;
                }
                return;
            }
            cursor += duration;
        }
        self.e.sequence = p.sequence.len() as i32 - 1;
        self.e.pattern = self.sequence_pattern(self.e.sequence);
        self.e.row = p.pattern_length(self.e.pattern) - 1;
        self.e.sequence_finished = true;
        self.e.row_remaining = 1e100;
        self.clear_scheduled();
    }

    /// Transport frame of the current sequence position and row.
    pub fn seek_frames(&self) -> f64 {
        let p = self.p;
        let e = &*self.e;
        let mut rows = 0.0f64;
        if e.song_mode {
            let mut n = 0;
            while n < e.sequence && n < p.sequence.len() as i32 {
                rows += p.pattern_rows(self.sequence_pattern(n));
                n += 1;
            }
        }
        rows += (e.row as f32 + if e.row % 2 != 0 { p.swing } else { 0.0 }) as f64;
        rows * e.sample_rate * 60.0 / p.bpm / p.lpb as f64
    }

    pub fn finish_song(&mut self) {
        let p = self.p;
        if self.song_loop_active() {
            self.loop_song_section(p.sequence.len() as i32, true, -1);
            return;
        }
        if p.loop_song {
            self.e.input_loop_beats += self.e.song_end_frames * p.bpm / (60.0 * self.e.sample_rate);
            self.push_song_tails();
            self.e.sequence = 0;
            self.e.pattern = self.valid_pattern(self.sequence_pattern(0));
            self.e.row = 0;
            self.e.row_remaining = 0.0;
            self.e.row_has_started = false;
            self.e.sequence_finished = false;
            self.e.transport_frames = 0.0;
            self.e.click_beat = -1;
            self.reset_track_state(true);
            self.all_off(false, true);
        } else {
            self.e.playing = false;
            self.clear_plugin_locks();
            self.all_off(false, true);
            self.clear_scheduled();
            self.e.sequence = p.sequence.len() as i32 - 1;
            self.e.row = p.pattern_length(self.e.pattern) - 1;
        }
    }

    #[inline]
    pub fn valid_pattern(&self, pattern: i32) -> i32 {
        clamp(pattern, 0, self.p.patterns.len() as i32 - 1)
    }

    /// Queued jump: Song mode resolves a sequence index (or the first occurrence of a pattern),
    /// Pattern mode a pattern. Ignored while stopped; Play/Stop/Seek/panic clear it.
    fn queue_jump(&mut self, mut target: i32, mut index: i32) {
        let p = self.p;
        self.clear_queued();
        if !self.e.playing || (target < 0 && index < 0) {
            return;
        }
        let count = p.sequence.len() as i32;
        if self.e.song_mode {
            if index < 0 || index >= count {
                index = -1;
                for n in 0..count {
                    if self.sequence_pattern(n) == target {
                        index = n;
                        break;
                    }
                }
            }
            if index < 0 {
                return;
            }
            self.e.queued_sequence = index;
            self.e.queued_pattern = self.valid_pattern(self.sequence_pattern(index));
        } else {
            if target < 0 {
                target = if index < count { self.sequence_pattern(index) } else { -1 };
            }
            if target < 0 || target >= p.patterns.len() as i32 {
                return;
            }
            self.e.queued_pattern = target;
        }
    }

    /// Song/Pattern mode change while playing, taken at the next pattern (or loop-range) end like
    /// a queued jump. To Pattern mode: loop `target` (the host makes it the active pattern). To
    /// Song mode: continue at sequence index `index`. Voices keep sounding; the position restarts
    /// exactly as a seek there would.
    fn queue_mode_switch(&mut self, song: bool, target: i32, index: i32) {
        let p = self.p;
        self.clear_queued();
        if !self.e.playing || song == self.e.song_mode {
            return;
        }
        if song {
            if index < 0 || index >= p.sequence.len() as i32 {
                return;
            }
            self.e.queued_sequence = index;
            self.e.queued_pattern = self.valid_pattern(self.sequence_pattern(index));
        } else {
            if target < 0 || target >= p.patterns.len() as i32 {
                return;
            }
            self.e.queued_pattern = target;
        }
        self.e.queued_mode = if song { 1 } else { 0 };
    }

    fn take_mode_switch(&mut self) -> bool {
        let p = self.p;
        let song = self.e.queued_mode == 1;
        let (target, index) = (self.e.queued_pattern, self.e.queued_sequence);
        self.clear_queued();
        self.e.song_mode = song;
        if song {
            self.e.sequence = clamp(index, 0, p.sequence.len() as i32 - 1);
            self.e.pattern = self.valid_pattern(self.sequence_pattern(self.e.sequence));
        } else {
            self.e.pattern = self.valid_pattern(target);
        }
        self.e.row = if self.pattern_loop_active() { self.e.loop_start_row } else { 0 };
        self.reset_pattern_clips();
        self.e.sequence_finished = false;
        self.reset_track_state(true);
        self.e.transport_frames = self.seek_frames();
        self.e.click_beat = -1;
        true
    }

    /// At a pattern (or loop-range) end: continue sample-accurately in the queued
    /// pattern/sequence position. Voices keep sounding, as at a natural pattern change.
    fn take_queued_jump(&mut self) -> bool {
        let p = self.p;
        if self.e.queued_pattern < 0 {
            return false;
        }
        if self.e.queued_mode >= 0 {
            return self.take_mode_switch();
        }
        let (target, index) = (self.e.queued_pattern, self.e.queued_sequence);
        self.e.queued_pattern = -1;
        self.e.queued_sequence = -1;
        if self.e.song_mode {
            if index < 0 || index >= p.sequence.len() as i32 {
                return false;
            }
            self.loop_song_section(self.e.sequence + 1, false, index);
            return true;
        }
        self.e.pattern = self.valid_pattern(target);
        self.e.row = if self.pattern_loop_active() { self.e.loop_start_row } else { 0 };
        self.reset_track_state(true);
        true
    }

    pub fn advance_row(&mut self) {
        let p = self.p;
        // Pattern loop range: at (or past) its last row, continue sample-accurately at its first
        // row.
        if self.pattern_loop_active() && self.e.row >= self.pattern_loop_end() {
            if self.take_queued_jump() {
                return;
            }
            self.e.row = self.e.loop_start_row;
            return;
        }
        self.e.row += 1;
        if self.e.row < p.pattern_length(self.e.pattern) {
            return;
        }
        self.e.row = 0;
        if self.take_queued_jump() {
            return;
        }
        if self.song_loop_active() && self.e.sequence == self.song_loop_last() {
            self.loop_song_section(self.e.sequence + 1, false, -1);
            return;
        }
        if !self.e.song_mode {
            return;
        }
        self.e.sequence += 1;
        if self.e.sequence >= p.sequence.len() as i32 {
            self.e.sequence = p.sequence.len() as i32 - 1;
            if self.e.transport_frames + 1e-6 < self.e.song_end_frames {
                self.e.sequence_finished = true;
                self.clear_plugin_locks();
                self.e.row = p.pattern_length(self.e.pattern) - 1;
                self.e.row_remaining = 1e100;
                return;
            }
            self.finish_song();
            return;
        }
        self.e.pattern = self.valid_pattern(self.sequence_pattern(self.e.sequence));
        self.reset_track_state(true);
    }

    /// `capture_transport` after the quiescent flush: the exact frame-domain cursor.
    pub fn capture_transport(&self) -> STTransportState {
        let e = &*self.e;
        STTransportState {
            playing: e.playing as i32,
            song_mode: e.song_mode as i32,
            pattern: e.pattern,
            row: e.row,
            sequence: e.sequence,
            row_has_started: e.row_has_started as i32,
            sequence_finished: e.sequence_finished as i32,
            row_remaining: e.row_remaining,
            row_duration: e.row_duration,
            transport_frames: e.transport_frames,
            recording: e.recording as i32,
            pattern_clip_frame: e.pattern_clip_frame,
            pattern_clip_pattern: e.pattern_clip_pattern,
            pattern_clip_looped: e.pattern_clip_looped as i32,
            pattern_clip_valid: e.pattern_clip_valid as i32,
        }
    }

    /// `restore_transport` after the quiescent flush: clear temporary voices, effects
    /// and schedules, restore the cursor/mode, resume only if `state.playing && resume`.
    pub fn restore_transport(&mut self, state: &STTransportState, resume: bool) {
        let p = self.p;
        let rate = self.e.sample_rate;
        self.e.reset_input_run();
        self.forget_early_inputs();
        self.e.out.suppress_events = true;
        self.all_off(true, false);
        self.clear_scheduled();
        self.e.clear_effects();
        self.tb.pitch_bend.fill(0.0);
        self.tb.mod_wheel.fill(0.0);
        self.clear_queued();
        self.e.count_in_total = 0;
        self.e.click_age = 1e9;
        self.e.click_beat = -1;
        self.e.playing = state.playing != 0 && resume;
        self.e.recording = state.recording != 0;
        self.e.song_mode = state.song_mode != 0;
        self.e.pattern = self.valid_pattern(state.pattern);
        self.e.row = clamp(state.row, 0, p.pattern_length(self.e.pattern) - 1);
        self.e.sequence = clamp(state.sequence, 0, p.sequence.len() as i32 - 1);
        self.e.row_has_started = state.row_has_started != 0;
        self.e.sequence_finished = state.sequence_finished != 0 && self.e.song_mode;
        // `1 + sign * swing` is formed in single precision, as in C++.
        let sign: f32 = if self.e.row & 1 != 0 { -1.0 } else { 1.0 };
        let default_duration = rate * 60.0 / p.bpm / p.lpb as f64 * (1.0f32 + sign * p.swing) as f64;
        self.e.row_duration = clamp(finite64(state.row_duration, default_duration), 1.0, rate * 6.0);
        self.e.row_remaining = if self.e.sequence_finished {
            1e100
        } else if self.e.row_has_started {
            clamp(finite64(state.row_remaining, 0.0), -1.0, self.e.row_duration)
        } else {
            0.0
        };
        self.e.transport_frames = clamp(finite64(state.transport_frames, 0.0), 0.0, 1e15);
        self.e.click_beat = if self.e.transport_frames > 0.0 {
            ((self.e.transport_frames - 1.0) * p.bpm * p.denominator as f64 / (rate * 240.0)).floor() as i64
        } else {
            -1
        };
        self.restore_pattern_clips(state);
        self.reset_track_state(false);
        self.e.out.suppress_events = false;
        self.sh.automation_refresh_requested.store(true, Ordering::Release);
        self.publish_transport();
        self.sh.peak_l.store(0.0, Ordering::Relaxed);
        self.sh.peak_r.store(0.0, Ordering::Relaxed);
        let store = self.sh.peak_store.load(Ordering::Acquire);
        if !store.is_null() {
            // SAFETY: peak stores live until the engine is destroyed.
            let store = unsafe { &*store };
            for n in 0..store.values.len() {
                store.store(n, 0.0);
            }
        }
    }
}

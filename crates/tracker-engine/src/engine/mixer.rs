//! The block renderer: `process` (per-sample sequencing and voices, each track's signal into its
//! bus), the metronome and count-in clicks on the return bus, and timeline audio (song clips,
//! pattern-mode clips and their printed tails, song-loop tails).
//!
//! A track's bus is the sum of its voices and clips and nothing else: faders, pan, mute, solo,
//! sends and the master belong to the host.

use super::state::*;
use super::tables::MAX_BLOCK;
use super::util::{cmax, cmin, lowest_bit, PI};
use super::voice::read_stereo;

impl Render {
    /// Render the next block of `frames` frames (1..=`MAX_BLOCK`) into the per-bus block
    /// buffers. `graph_start` is the host's timestamp of the first frame. Events carry frame
    /// offsets from the first frame.
    pub(crate) fn process(&mut self, sh: &Shared, frames: u32, graph_start: u64) {
        self.out.frame_offset = 0;
        self.refresh(sh);
        // SAFETY: called on the render thread; the view ends with this call, before the next
        // refresh.
        let mut rt = unsafe { Rt::attach(self, sh) };
        rt.process_block(frames, graph_start);
    }
}

impl<'a> Rt<'a> {
    pub fn reset_pattern_clips(&mut self) {
        let e = &mut *self.e;
        e.pattern_clip_valid = false;
        e.pattern_clip_looped = false;
        e.pattern_clip_row = -1;
        for tail in e.pattern_tails.iter_mut() {
            tail.active = false;
        }
        for tail in e.song_tails.iter_mut() {
            tail.active = false;
        }
    }

    /// Whole-song loop: each song tail clip starts sounding its printed tail at the new pass's
    /// frame 0, as the live chain would ring over the loop point. The oldest tail yields when
    /// all slots ring.
    pub fn push_song_tails(&mut self) {
        let tails = &mut self.e.song_tails;
        for clip in 0..self.p.song_tail_clips.len() {
            let mut slot = tails.iter().position(|tail| !tail.active);
            if slot.is_none() {
                let mut oldest = 0;
                for (index, tail) in tails.iter().enumerate() {
                    if tail.position > tails[oldest].position {
                        oldest = index;
                    }
                }
                slot = Some(oldest);
            }
            if let Some(tail) = slot.and_then(|index| tails.get_mut(index)) {
                tail.active = true;
                tail.clip = clip as i32;
                tail.position = 0;
            }
        }
    }

    fn mix_song_tails(&mut self) {
        let p = self.p;
        let sample_rate = self.e.sample_rate;
        for index in 0..self.e.song_tails.len() {
            let tail = self.e.song_tails[index];
            if !tail.active {
                continue;
            }
            let Some(clip) = p.song_tail_clips.get(tail.clip as u32 as usize) else {
                self.e.song_tails[index].active = false;
                continue;
            };
            let Some(sample) = p.asset_sample(clip.asset) else {
                self.e.song_tails[index].active = false;
                continue;
            };
            let pos = clip.start as f64
                + (if sample.rate == sample_rate {
                    tail.position as f64
                } else {
                    tail.position as f64 * sample.rate / sample_rate
                });
            if pos >= sample.l.len() as f64 {
                self.e.song_tails[index].active = false;
                continue;
            }
            self.e.song_tails[index].position += 1;
            if clip.track >= p.track_count {
                continue;
            }
            let (l, r) = read_stereo(sample, pos, clip.gain);
            self.add_channel(clip.track, l, r);
        }
    }

    /// Frames from the pattern's row 0 to row `r`.
    fn pattern_row_frame(&self, r: i32) -> f64 {
        let p = self.p;
        p.beat_for_row(r) * 60.0 / p.bpm * self.e.sample_rate
    }

    fn push_pattern_tail(&mut self, pattern: i32, looped: bool, position: i64) {
        let tails = &mut self.e.pattern_tails;
        let mut slot = tails.iter().position(|tail| !tail.active);
        if slot.is_none() {
            let mut oldest = 0;
            for (index, tail) in tails.iter().enumerate() {
                if tail.position > tails[oldest].position {
                    oldest = index;
                }
            }
            slot = Some(oldest);
        }
        if let Some(tail) = slot.and_then(|index| tails.get_mut(index)) {
            *tail = PatternTail { active: true, looped, pattern, position };
        }
    }

    /// Called at every Pattern-mode row trigger. A natural wrap (last row -> row 0) restarts the
    /// pass; the same pattern then plays its steady-state loop print, another pattern keeps the
    /// previous print's tail sounding. Any other jump (Play, Seek, loop range, pattern change)
    /// re-aligns to the row's start frame.
    pub fn note_pattern_row(&mut self) {
        let p = self.p;
        let e = &mut *self.e;
        let natural = e.pattern_clip_valid && e.row == 0 && e.pattern_clip_row == e.pattern_clip_length - 1;
        if natural {
            if e.pattern == e.pattern_clip_pattern {
                e.pattern_clip_looped = true;
            } else {
                let (previous, looped, frame) = (e.pattern_clip_pattern, e.pattern_clip_looped, e.pattern_clip_frame);
                if p.pattern_clips.iter().any(|clip| clip.pattern == previous) {
                    self.push_pattern_tail(previous, looped, frame);
                }
                self.e.pattern_clip_looped = false;
            }
            self.e.pattern_clip_frame = 0;
        } else if !(e.pattern_clip_valid && e.pattern == e.pattern_clip_pattern && e.row == e.pattern_clip_row + 1) {
            if e.pattern != e.pattern_clip_pattern {
                e.pattern_clip_looped = false;
            }
            let row = e.row;
            self.e.pattern_clip_frame = (self.pattern_row_frame(row) - 1e-6).ceil() as i64;
        }
        let e = &mut *self.e;
        e.pattern_clip_pattern = e.pattern;
        e.pattern_clip_row = e.row;
        e.pattern_clip_length = p.pattern_length(e.pattern);
        e.pattern_clip_valid = true;
    }

    /// Add one frame of a voice or clip to its track's bus. The first signal of a block clears
    /// the bus's block; a bus nothing is added to stays unwritten and reads as silent
    /// (`BlockView::track`). Voices and clips add in the order they are visited, so a bus is
    /// the sum of its inputs in that order.
    #[inline(always)]
    pub fn add_channel(&mut self, channel: i32, l: f32, r: f32) {
        let c = channel as u32 as usize;
        let audible = self.channel_enabled(channel);
        let (frame, frames) = (self.e.block_frame as usize, self.e.block_frames as usize);
        let tb = &mut *self.tb;
        let Some(touched) = tb.bus_touched.get_mut(c) else { return };
        let fresh = *touched == 0;
        *touched = 1;
        let (bl, br) = tb.bus_pair(c);
        if fresh {
            if let (Some(a), Some(b)) = (bl.get_mut(..frames), br.get_mut(..frames)) {
                a.fill(0.0);
                b.fill(0.0);
            }
        }
        if audible {
            if let (Some(a), Some(b)) = (bl.get_mut(frame), br.get_mut(frame)) {
                *a += l;
                *b += r;
            }
        }
    }

    /// One frame of every clip printed for `pattern`; false when all of them have ended.
    fn mix_pattern_clip(&mut self, pattern: i32, looped: bool, position: i64) -> bool {
        let p = self.p;
        let sample_rate = self.e.sample_rate;
        let mut remaining = false;
        if position < 0 {
            return false;
        }
        for clip in &p.pattern_clips {
            if clip.pattern != pattern {
                continue;
            }
            let asset = if looped && clip.loop_asset >= 0 { clip.loop_asset } else { clip.head_asset };
            let Some(sample) = p.asset_sample(asset) else { continue };
            let pos =
                if sample.rate == sample_rate { position as f64 } else { position as f64 * sample.rate / sample_rate };
            if pos >= sample.l.len() as f64 {
                continue;
            }
            remaining = true;
            if clip.track >= p.track_count {
                continue;
            }
            let (l, r) = read_stereo(sample, pos, clip.gain);
            self.add_channel(clip.track, l, r);
        }
        remaining
    }

    fn mix_pattern_clips(&mut self) {
        if self.p.pattern_clips.is_empty() {
            for tail in self.e.pattern_tails.iter_mut() {
                tail.active = false;
            }
        } else {
            if self.e.pattern_clip_valid {
                self.mix_pattern_clip(self.e.pattern_clip_pattern, self.e.pattern_clip_looped, self.e.pattern_clip_frame);
            }
            for index in 0..self.e.pattern_tails.len() {
                let tail = self.e.pattern_tails[index];
                if !tail.active {
                    continue;
                }
                if self.mix_pattern_clip(tail.pattern, tail.looped, tail.position) {
                    self.e.pattern_tails[index].position += 1;
                } else {
                    self.e.pattern_tails[index].active = false;
                }
            }
        }
        self.e.pattern_clip_frame = self.e.pattern_clip_frame.wrapping_add(1);
    }

    /// Timeline audio for this sample: song clips (Song mode) or pattern prints (Pattern mode).
    fn mix_clips(&mut self) {
        if !self.e.playing {
            return;
        }
        if !self.e.song_mode {
            self.mix_pattern_clips();
            return;
        }
        self.mix_song_tails();
        let p = self.p;
        if p.audio_clips.is_empty() {
            return;
        }
        let sample_rate = self.e.sample_rate;
        let transport_frames = self.e.transport_frames;
        let seconds = transport_frames / sample_rate;
        for clip in &p.audio_clips {
            if clip.muted || clip.track >= p.track_count {
                continue;
            }
            let Some(sample) = p.asset_sample(clip.asset) else { continue };
            let elapsed = seconds - clip.start_beat * 60.0 / p.bpm;
            if elapsed < 0.0 {
                continue;
            }
            let duration = p.clip_seconds(clip);
            if elapsed >= duration {
                continue;
            }
            let pos = (clip.source_offset + elapsed) * sample.rate;
            if pos >= sample.l.len() as f64 {
                continue;
            }
            let mut gain = clip.gain;
            if clip.fade_in > 0.0 {
                gain *= cmin(1.0, elapsed / clip.fade_in as f64) as f32;
            }
            if clip.fade_out > 0.0 {
                gain *= cmin(1.0, (duration - elapsed) / clip.fade_out as f64) as f32;
            }
            let (l, r) = read_stereo(sample, pos, gain);
            self.add_channel(clip.track, l, r);
        }
    }

    /// One click sample of the shared click oscillator, scaled by `volume`.
    #[inline]
    fn click_sample(&mut self, volume: f32, wet_l: &mut f32, wet_r: &mut f32) {
        let e = &mut *self.e;
        if e.click_age < e.sample_rate * 0.03 {
            // double * float * float, evaluated in double.
            let click = (e.click_phase.sin()
                * ((-e.click_age / (e.sample_rate * 0.005)) as f32).exp() as f64
                * volume as f64) as f32;
            *wet_l += click;
            *wet_r += click;
            e.click_phase += 2.0 * PI * (if e.click_accent { 1500.0 } else { 1000.0 }) / e.sample_rate;
            e.click_age += 1.0;
        }
    }

    fn metronome_click(&mut self, wet_l: &mut f32, wet_r: &mut f32) {
        let p = self.p;
        let e = &mut *self.e;
        // The meter's beat (a quarter note for x/4) the transport frame lies in.
        let beat = (e.transport_frames * p.bpm * p.denominator as f64 / (e.sample_rate * 240.0)) as i64;
        if beat != e.click_beat {
            e.click_beat = beat;
            e.click_age = 0.0;
            e.click_phase = 0.0;
            e.click_accent = beat % cmax(1, p.numerator) as i64 == 0;
        }
        self.click_sample(p.metronome_volume, wet_l, wet_r);
    }

    fn count_in_click_sample(&mut self, wet_l: &mut f32, wet_r: &mut f32) {
        let volume = if self.p.metronome_volume > 0.0 { self.p.metronome_volume } else { 0.3 };
        self.click_sample(volume, wet_l, wet_r);
    }

    fn process_block(&mut self, frames: u32, graph_start: u64) {
        let p = self.p;
        let return_bus = self.tb.base as usize;
        // Bus block buffers hold MAX_BLOCK frames.
        let frames = cmin(frames, MAX_BLOCK as u32);
        // Only a bus that receives signal in this block is written (`BlockView::track` reports
        // the others as `None`); the return bus always is.
        self.tb.bus_touched.fill(0);
        if let Some(touched) = self.tb.bus_touched.get_mut(return_bus) {
            *touched = 1;
        }
        self.e.block_frames = frames;
        for f in 0..frames {
            let frame = f as usize;
            self.e.out.frame_offset = f as i32;
            self.e.block_frame = f;
            if self.e.count_in_total > 0 {
                self.step_count_in();
            }
            if self.e.playing
                && self.e.song_mode
                && self.e.sequence_finished
                && self.e.transport_frames >= self.e.song_end_frames
            {
                self.finish_song();
            }
            if self.e.playing && !self.e.sequence_finished && self.e.row_remaining <= 0.0 {
                if self.e.row_has_started {
                    self.advance_row();
                }
                if self.e.playing && !self.e.sequence_finished {
                    self.trigger_row();
                }
            }
            if f == 0 {
                // What the block's first frame plays: known once this frame's commands, count-in
                // step, song end and row start have been applied, before it advances.
                self.e.block_song_frame = if self.e.playing { Some(self.e.transport_frames as i64) } else { None };
            }
            if self.e.pending_input_count != 0 {
                self.finish_input_acks(graph_start.wrapping_add(f as u64));
            }
            self.automation();
            self.start_due_hits();

            // Ascending set bits preserve summation order while skipping inactive slots.
            for group in 0..VOICE_GROUPS {
                let mut active = self.e.active_voice_masks[group];
                while active != 0 {
                    let bit = lowest_bit(active);
                    active &= active - 1;
                    let index = group * 64 + bit;
                    let (l, r) = self.voice_sample(index);
                    if let Some(v) = self.e.voices.get(index) {
                        if !v.external {
                            self.add_channel(v.track, l, r);
                        }
                    }
                }
            }
            self.mix_clips();
            // The return bus: the metronome and the count-in clicks.
            let (mut wet_l, mut wet_r) = (0.0f32, 0.0f32);
            if p.metronome && self.e.playing {
                self.metronome_click(&mut wet_l, &mut wet_r);
            }
            if self.e.count_in_total > 0 {
                self.count_in_click_sample(&mut wet_l, &mut wet_r);
            }
            let (bl, br) = self.tb.bus_pair(return_bus);
            if let (Some(a), Some(b)) = (bl.get_mut(frame), br.get_mut(frame)) {
                *a = wet_l;
                *b = wet_r;
            }
            self.e.total_frames += 1.0;
            if self.e.playing {
                self.e.row_remaining -= 1.0;
                self.e.transport_frames += 1.0;
            }
        }
        self.publish_voice_positions();
        self.publish_transport();
    }
}

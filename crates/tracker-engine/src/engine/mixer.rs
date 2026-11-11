//! The block renderer (ported from the C++ engine's `TrackerEngine.cpp`): `process` (per-sample sequencing,
//! voices, channel mix, peaks), the legacy send returns (stereo cross-feedback delay, pre-delay,
//! eight-comb/four-allpass reverb), the metronome and count-in clicks, and timeline audio
//! (`mixClips`, pattern-mode clips and their printed tails, song-loop tails).

use super::state::*;
use super::tables::MAX_BLOCK;
use super::util::{clamp, cmax, cmin, lowest_bit, PI};
use super::voice::read_stereo;
use std::sync::atomic::Ordering;

impl Render {
    /// Render `frames` frames. `buses` renders into the per-bus block buffers (a host's block)
    /// instead of only the stereo mix; `graph_start` is then the host's timestamp of the first
    /// frame. Events carry frame offsets from the first frame.
    pub(crate) fn process(
        &mut self,
        sh: &Shared,
        out_l: Option<&mut [f32]>,
        out_r: Option<&mut [f32]>,
        frames: u32,
        buses: bool,
        graph_start: u64,
    ) {
        self.out.frame_offset = 0;
        self.refresh(sh);
        // SAFETY: called on the render thread; the view ends with this call, before the next
        // refresh.
        let mut rt = unsafe { Rt::attach(self, sh) };
        rt.process_block(out_l, out_r, frames, buses, graph_start);
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
            if clip.track >= p.track_count || (p.host_mixing && !self.computational_track_enabled(clip.track)) {
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

    /// Accumulate into a channel's per-sample input and mark it for this sample's mix.
    ///
    /// Raw host mixing of a block render: a channel's bus is exactly its summed input, so voices
    /// and clips add straight into the bus block (same order, same sums) instead of a per-sample
    /// channel pass; peaks and the mix are taken from the buses per block.
    #[inline(always)]
    pub fn add_channel(&mut self, channel: i32, l: f32, r: f32) {
        let c = channel as u32 as usize;
        let tb = &mut *self.tb;
        if self.e.direct_bus {
            let (frame, frames) = (self.e.direct_frame as usize, self.e.direct_frames as usize);
            let Some(touched) = tb.bus_touched.get_mut(c) else { return };
            let fresh = *touched == 0;
            *touched = 1;
            let audible = tb.host_audible.get(c).is_some_and(|&flag| flag != 0);
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
            return;
        }
        if let (Some(a), Some(b), Some(word)) = (tb.left.get_mut(c), tb.right.get_mut(c), tb.touched.get_mut(c / 64)) {
            *a += l;
            *b += r;
            *word |= 1u64 << (c % 64);
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
            if clip.track >= p.track_count || (p.host_mixing && !self.computational_track_enabled(clip.track)) {
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
            if clip.muted
                || clip.track >= p.track_count
                || (p.host_mixing && !self.computational_track_enabled(clip.track))
            {
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
        let p = self.p;
        let e = &mut *self.e;
        if e.click_age < e.sample_rate * 0.03 {
            let master: f32 = if p.host_mixing { 1.0 } else { p.master };
            // double * float * float * float, evaluated in double as in C++.
            let click = (e.click_phase.sin()
                * ((-e.click_age / (e.sample_rate * 0.005)) as f32).exp() as f64
                * volume as f64
                * master as f64) as f32;
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

    /// The legacy send returns for one sample: delay input (`del`), reverb input (`rev`) ->
    /// wet left/right, master gain applied.
    #[inline(always)]
    fn send_returns(&mut self, del_l: f32, del_r: f32, mut rev_l: f32, mut rev_r: f32) -> (f32, f32) {
        let p = self.p;
        let e = &mut *self.e;
        let size = e.delay_l.len();
        if size < 2 || e.delay_r.len() != size {
            return (0.0, 0.0);
        }
        let seconds = if p.delay_beats > 0.0 { p.delay_beats * 60.0 / cmax(1.0, p.bpm) } else { p.delay_time as f64 };
        let delay_samples = clamp(seconds * e.sample_rate, 1.0, (size - 1) as f64);
        let mut read = e.delay_pos as f64 - delay_samples;
        // A tiny negative read rounds up to exactly size(): that is position 0, not one past the
        // end.
        if read < 0.0 {
            read += size as f64;
            if read >= size as f64 {
                read = 0.0;
            }
        }
        let ia = cmin(read as usize, size - 1);
        let ib = (ia + 1) % size;
        let fract = (read - ia as f64) as f32;
        let mut wet_dl = e.delay_l[ia] + (e.delay_l[ib] - e.delay_l[ia]) * fract;
        let mut wet_dr = e.delay_r[ia] + (e.delay_r[ib] - e.delay_r[ia]) * fract;
        if p.delay_tone_coef < 1.0 {
            e.delay_tone_l += p.delay_tone_coef * (wet_dl - e.delay_tone_l);
            e.delay_tone_r += p.delay_tone_coef * (wet_dr - e.delay_tone_r);
            wet_dl = e.delay_tone_l;
            wet_dr = e.delay_tone_r;
        }
        let position = cmin(e.delay_pos, size - 1);
        e.delay_l[position] = del_l + wet_dr * p.delay_feedback;
        e.delay_r[position] = del_r + wet_dl * p.delay_feedback;
        e.delay_pos = position + 1;
        if e.delay_pos == size {
            e.delay_pos = 0;
        }
        if p.reverb_predelay > 0.0 {
            let length = e.pre_l.len();
            if length > 0 && e.pre_r.len() == length {
                let lag = cmin(length - 1, (p.reverb_predelay as f64 * e.sample_rate) as usize);
                let position = cmin(e.pre_pos, length - 1);
                e.pre_l[position] = rev_l;
                e.pre_r[position] = rev_r;
                let read = (position + length - lag) % length;
                rev_l = e.pre_l[read];
                rev_r = e.pre_r[read];
                e.pre_pos = position + 1;
                if e.pre_pos == length {
                    e.pre_pos = 0;
                }
            }
        }
        let (mut wet_rl, mut wet_rr) = (0.0f32, 0.0f32);
        let feedback = 0.28 + p.reverb_size * 0.67;
        let (in_l, in_r) = ((rev_l + rev_r * 0.2) * 0.075, (rev_r + rev_l * 0.2) * 0.075);
        for c in 0..8 {
            wet_rl += e.comb_l[c].process(in_l, feedback, p.reverb_damping);
            wet_rr += e.comb_r[c].process(in_r, feedback, p.reverb_damping);
        }
        for a in 0..4 {
            wet_rl = e.ap_l[a].process(wet_rl);
            wet_rr = e.ap_r[a].process(wet_rr);
        }
        (
            (wet_dl * p.delay_mix + wet_rl * p.reverb_mix) * p.master,
            (wet_dr * p.delay_mix + wet_rr * p.reverb_mix) * p.master,
        )
    }

    fn process_block(
        &mut self,
        mut out_l: Option<&mut [f32]>,
        mut out_r: Option<&mut [f32]>,
        frames: u32,
        buses: bool,
        graph_start: u64,
    ) {
        let p = self.p;
        let return_bus = self.tb.base as usize;
        // Bus block buffers hold MAX_BLOCK frames; block renders never exceed that.
        let frames = if buses { cmin(frames, MAX_BLOCK as u32) } else { frames };
        let block = frames as usize;
        // Only channels that receive signal in a sample are mixed; a bus that receives nothing
        // in this block is not written at all (`BlockView::track` reports it as `None`).
        if buses {
            self.tb.bus_touched.fill(0);
            if let Some(touched) = self.tb.bus_touched.get_mut(return_bus) {
                *touched = 1;
            }
        }
        let (mut block_peak_l, mut block_peak_r) = (0.0f32, 0.0f32);
        self.tb.peaks.fill(0.0);
        let solo = p.any_solo;
        let direct_bus = buses && p.host_mixing;
        self.e.direct_bus = direct_bus;
        self.e.direct_frames = frames;
        if direct_bus {
            for t in 0..self.tb.channels {
                let audible = self.channel_enabled(t) && self.computational_track_enabled(t);
                if let Some(flag) = self.tb.host_audible.get_mut(t as usize) {
                    *flag = audible as u8;
                }
            }
        }
        for f in 0..frames {
            let frame = f as usize;
            self.e.out.frame_offset = f as i32;
            self.e.direct_frame = f;
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
            if self.e.pending_input_count != 0 {
                self.finish_input_acks(true, buses, graph_start.wrapping_add(f as u64));
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
            let (mut mix_l, mut mix_r) = (0.0f32, 0.0f32);
            let (mut del_l, mut del_r, mut rev_l, mut rev_r) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            if direct_bus {
                // Only the return (metronome) remains per sample; channels are already in their
                // buses.
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
                continue;
            }
            // Visit only channels that received a voice or clip this sample, in ascending
            // channel order (the historical summation order); silent channels add nothing.
            let tb = &mut *self.tb;
            for word in 0..tb.touched.len() {
                let mut bits = tb.touched[word];
                tb.touched[word] = 0;
                while bits != 0 {
                    let t = word * 64 + lowest_bit(bits);
                    bits &= bits - 1;
                    let (Some(left), Some(right)) = (tb.left.get_mut(t), tb.right.get_mut(t)) else { continue };
                    let (in_l, in_r) = (*left, *right);
                    *left = 0.0;
                    *right = 0.0;
                    let enabled = (t as i32) < p.track_count;
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    if p.host_mixing {
                        if enabled && p.track_computational(t as i32) {
                            l = in_l;
                            r = in_r;
                        }
                    } else {
                        if let (true, Some(mix), Some(s)) = (enabled, p.tracks.get(t), tb.track_state.get(t)) {
                            if mix.mute == 0 && (!solo || mix.solo != 0) {
                                let g = s.volume;
                                let pan = clamp(s.pan, -1.0, 1.0);
                                l = in_l * g * (1.0 - pan).sqrt();
                                r = in_r * g * (1.0 + pan).sqrt();
                                del_l += l * s.delay;
                                del_r += r * s.delay;
                                rev_l += l * s.reverb;
                                rev_r += r * s.reverb;
                            }
                        }
                        l *= p.master;
                        r *= p.master;
                    }
                    if let Some(peak) = tb.peaks.get_mut(t) {
                        *peak = cmax(*peak, cmax(l.abs(), r.abs()));
                    }
                    mix_l += l;
                    mix_r += r;
                    if buses {
                        let fresh = match tb.bus_touched.get_mut(t) {
                            Some(touched) => {
                                let fresh = *touched == 0;
                                *touched = 1;
                                fresh
                            }
                            None => continue,
                        };
                        let (bl, br) = tb.bus_pair(t);
                        if fresh {
                            if let (Some(a), Some(b)) = (bl.get_mut(..block), br.get_mut(..block)) {
                                a.fill(0.0);
                                b.fill(0.0);
                            }
                        }
                        if let (Some(a), Some(b)) = (bl.get_mut(frame), br.get_mut(frame)) {
                            *a = l;
                            *b = r;
                        }
                    }
                }
            }
            let (mut wet_l, mut wet_r) = (0.0f32, 0.0f32);
            if !p.host_mixing {
                (wet_l, wet_r) = self.send_returns(del_l, del_r, rev_l, rev_r);
            }
            if p.metronome && self.e.playing {
                self.metronome_click(&mut wet_l, &mut wet_r);
            }
            if self.e.count_in_total > 0 {
                self.count_in_click_sample(&mut wet_l, &mut wet_r);
            }
            if buses {
                let (bl, br) = self.tb.bus_pair(return_bus);
                if let (Some(a), Some(b)) = (bl.get_mut(frame), br.get_mut(frame)) {
                    *a = wet_l;
                    *b = wet_r;
                }
            }
            mix_l += wet_l;
            mix_r += wet_r;
            // Bounded safety clip only above full scale; normal gain remains linear.
            mix_l = clamp(mix_l, -1.0, 1.0);
            mix_r = clamp(mix_r, -1.0, 1.0);
            if let Some(slot) = out_l.as_deref_mut().and_then(|out| out.get_mut(frame)) {
                *slot = mix_l;
            }
            if let Some(slot) = out_r.as_deref_mut().and_then(|out| out.get_mut(frame)) {
                *slot = mix_r;
            }
            block_peak_l = cmax(block_peak_l, mix_l.abs());
            block_peak_r = cmax(block_peak_r, mix_r.abs());
            self.e.total_frames += 1.0;
            if self.e.playing {
                self.e.row_remaining -= 1.0;
                self.e.transport_frames += 1.0;
            }
        }
        if direct_bus {
            // Per-channel peaks and the summed mix, as the per-sample channel pass computes
            // them: channels in ascending order (a bus untouched at a sample adds exactly 0),
            // then the return.
            self.e.direct_bus = false;
            let e = &mut *self.e;
            let tb = &mut *self.tb;
            let (Some(mix_l), Some(mix_r)) = (e.direct_mix_l.get_mut(..block), e.direct_mix_r.get_mut(..block)) else {
                return;
            };
            mix_l.fill(0.0);
            mix_r.fill(0.0);
            for b in 0..return_bus {
                if tb.bus_touched.get(b).is_none_or(|&touched| touched == 0) {
                    continue;
                }
                let (Some(bl), Some(br)) = (tb.bus_left(b).get(..block), tb.bus_right(b).get(..block)) else { continue };
                let mut peak = 0.0f32;
                for f in 0..block {
                    mix_l[f] += bl[f];
                    mix_r[f] += br[f];
                    peak = cmax(peak, cmax(bl[f].abs(), br[f].abs()));
                }
                if let Some(slot) = tb.peaks.get_mut(b) {
                    *slot = peak;
                }
            }
            if let (Some(wl), Some(wr)) = (tb.bus_left(return_bus).get(..block), tb.bus_right(return_bus).get(..block)) {
                for f in 0..block {
                    let l = clamp(mix_l[f] + wl[f], -1.0, 1.0);
                    let r = clamp(mix_r[f] + wr[f], -1.0, 1.0);
                    block_peak_l = cmax(block_peak_l, l.abs());
                    block_peak_r = cmax(block_peak_r, r.abs());
                }
            }
        }
        if self.e.pending_input_count != 0 {
            self.finish_input_acks(false, false, 0);
        }
        self.publish_voice_positions();
        self.publish_transport();
        let sh = self.sh;
        sh.peak_l.store(cmax(block_peak_l, sh.peak_l.load(Ordering::Relaxed) * 0.85), Ordering::Relaxed);
        sh.peak_r.store(cmax(block_peak_r, sh.peak_r.load(Ordering::Relaxed) * 0.85), Ordering::Relaxed);
        let store = sh.peak_store.load(Ordering::Acquire);
        if !store.is_null() {
            // SAFETY: peak stores are allocated by the control thread and live until the engine
            // is destroyed (they are only ever replaced by larger ones, never freed).
            let store = unsafe { &*store };
            let count = cmin(store.values.len(), self.tb.peaks.len());
            for t in 0..count {
                store.store(t, cmax(self.tb.peaks[t], store.load(t) * 0.85));
            }
        }
    }
}

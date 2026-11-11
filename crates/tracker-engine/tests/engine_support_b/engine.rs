//! An owned engine for tests (`tracker_engine::engine::Engine`): one method per operation the
//! C++ test programs called, each a plain forward to the engine's Rust API (the `Control` methods
//! reached through the control half, and the render half's own methods).

use tracker_engine::engine::control::Control;
use tracker_engine::engine::edits::Pcm;
use tracker_engine::engine::host;
use tracker_engine::engine::state::Shared;
use tracker_engine::engine::*;

pub struct Engine {
    e: host::Engine,
    /// See [`pull_block`].
    block: Option<(u64, usize)>,
}

/// The buses of an engine as the tests pull them: one bus of a block at a time.
///
/// `block` is the range (start, frames) rendered last. A pull of that range reads the block
/// again (the tests take one bus after another); any other pull renders the next block, which
/// must follow the previous one and be at most `MAX_BLOCK` frames long. `None` after a direct
/// render: the next block may start at any time.
fn pull_block(render: &mut host::RenderHalf, block: &mut Option<(u64, usize)>, time: u64, frames: usize) {
    if *block == Some((time, frames)) {
        return;
    }
    assert!((1..=tracker_engine::engine::tables::MAX_BLOCK).contains(&frames), "a pull is one block of 1..=MAX_BLOCK frames");
    if let Some((start, count)) = *block {
        assert_eq!(time, start + count as u64, "blocks follow one another");
    }
    assert_eq!(render.render_block(time, frames as u32) as usize, frames);
    *block = Some((time, frames));
}

/// Copies one bus of the block rendered last: a track's own number, or the return bus that
/// follows the tracks (never below 16). A bus nothing played on, and one the project lacks,
/// is silent.
fn copy_bus(render: &mut host::RenderHalf, bus: i32, left: &mut [f32], right: &mut [f32]) {
    let view = render.block();
    let source = if bus == tracker_engine::engine::return_bus(view.track_count() as i32) {
        Some(view.return_bus())
    } else if bus >= 0 {
        view.track(bus as usize)
    } else {
        None
    };
    match source {
        Some((l, r)) => {
            left.copy_from_slice(&l[..left.len()]);
            right.copy_from_slice(&r[..right.len()]);
        }
        None => {
            left.fill(0.0);
            right.fill(0.0);
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// A chord as the setters take it: `None` for an empty list.
fn chord_list(chord: &[i32]) -> Option<&[i32]> {
    (!chord.is_empty()).then_some(chord)
}

impl Engine {
    /// An engine at 48 kHz.
    pub fn new() -> Engine {
        Engine::with_rate(48000.0)
    }

    /// An engine at `rate`; the C++ fixtures assert that it exists.
    pub fn with_rate(rate: f64) -> Engine {
        Engine { e: host::Engine::new(rate).expect("an engine at a supported rate"), block: None }
    }

    /// The control state, the shared state its methods take and the engine's sample rate.
    fn control(&mut self) -> (&mut Control, &Shared, f64) {
        self.e.control().parts()
    }

    pub fn sample_rate(&self) -> f64 {
        self.e.sample_rate()
    }

    // ---- update transactions ----

    pub fn begin_update(&mut self) {
        let (c, sh, _) = self.control();
        c.begin_update(sh);
    }

    pub fn clear_project(&mut self) {
        self.control().0.clear_project();
    }

    /// `commit_update`: a failed commit keeps the previous project (and the draft).
    pub fn commit_update(&mut self) {
        let _ = self.commit_update_checked();
    }

    /// `commit_update_checked`: the error text instead of `0` + buffer.
    pub fn commit_update_checked(&mut self) -> Result<(), String> {
        let (c, sh, _) = self.control();
        c.commit(sh)
    }

    pub fn cancel_update(&mut self) {
        self.control().0.cancel_update();
    }

    pub fn collect(&mut self) {
        let (c, sh, _) = self.control();
        c.collect(sh);
    }

    // ---- project settings ----

    pub fn set_timing(&mut self, bpm: f64, lines_per_beat: i32, swing: f32, master_volume: f32) {
        self.control().0.set_timing(bpm, lines_per_beat, swing, master_volume);
    }

    pub fn set_metronome(&mut self, enabled: bool, numerator: i32, denominator: i32, volume: f32) {
        self.control().0.set_metronome(enabled, numerator, denominator, volume);
    }

    pub fn set_track_count(&mut self, count: i32) -> bool {
        self.control().0.set_track_count(count)
    }

    pub fn set_track(&mut self, index: i32, track: STTrack) {
        self.control().0.set_track(index, track);
    }

    pub fn set_track_voice(&mut self, track: i32, choke_group: i32, choke_scope: i32, voice_mode: i32) -> bool {
        self.control().0.set_track_voice(track, choke_group, choke_scope, voice_mode)
    }

    pub fn set_host_mixing(&mut self, enabled: bool, computational_mask: u32) {
        // Bit t of the mask is track t for t < 32; tracks from 32 on are enabled.
        self.control().0.set_host_mixing_tracks(enabled, &[computational_mask as u64 | !0xffff_ffffu64], true);
    }

    pub fn set_host_mixing_tracks(&mut self, enabled: bool, words: &[u64], enabled_beyond: bool) {
        self.control().0.set_host_mixing_tracks(enabled, words, enabled_beyond);
    }

    pub fn set_instrument_count(&mut self, count: i32) -> bool {
        self.control().0.set_instrument_count(count)
    }

    pub fn set_instrument(&mut self, index: i32, instrument: STInstrument) {
        self.control().0.set_instrument(index, instrument);
    }

    pub fn set_sequence(&mut self, patterns: &[i32], loop_song: bool, active_pattern: i32) {
        self.control().0.set_sequence(patterns, loop_song, active_pattern);
    }

    pub fn set_effects(
        &mut self,
        delay_seconds: f32,
        delay_feedback: f32,
        delay_mix: f32,
        reverb_size: f32,
        reverb_damping: f32,
        reverb_mix: f32,
    ) {
        self.control().0.set_effects(delay_seconds, delay_feedback, delay_mix, reverb_size, reverb_damping, reverb_mix);
    }

    pub fn set_song_loop(&mut self, start_index: i32, end_index: i32) {
        self.control().0.set_song_loop(start_index, end_index);
    }

    pub fn set_send_effects(&mut self, delay_beats: f64, delay_tone: f32, reverb_predelay: f32) {
        let (c, _, rate) = self.control();
        c.set_send_effects(rate, delay_beats, delay_tone, reverb_predelay);
    }

    pub fn set_host_automation_map(&mut self, map: u64) {
        self.control().0.set_host_automation_map(map);
    }

    // ---- patterns, steps and automation ----

    pub fn set_pattern(&mut self, index: i32, length: i32) {
        self.control().0.set_pattern(index, length);
    }

    pub fn reuse_pattern(&mut self, index: i32) -> bool {
        self.control().0.reuse_pattern(index)
    }

    pub fn pattern_epoch(&mut self) -> u64 {
        self.control().0.pattern_epoch()
    }

    /// `set_step(e, pattern, track, row, step, chord, count)`: an empty `chord` is the
    /// C call's `nullptr, 0`.
    pub fn set_step(&mut self, pattern: i32, track: i32, row: i32, step: STStep, chord: &[i32]) {
        self.control().0.set_step(pattern, track, row, step, chord_list(chord));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_plugin_step_fx(
        &mut self,
        pattern: i32,
        track: i32,
        row: i32,
        column: i32,
        slot: i32,
        host_destination: i32,
        address: u64,
        value: f64,
    ) -> bool {
        self.control().0.set_plugin_step_fx(pattern, track, row, column, slot, host_destination, address, value)
    }

    pub fn clear_step(&mut self, pattern: i32, track: i32, row: i32) {
        self.control().0.clear_step(pattern, track, row);
    }

    pub fn set_step_column(&mut self, pattern: i32, track: i32, row: i32, column: i32, step: STStep) {
        self.control().0.set_step_column(pattern, track, row, column, step);
    }

    pub fn clear_automation(&mut self, pattern: i32) {
        self.control().0.clear_automation(pattern);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_automation_point(
        &mut self,
        pattern: i32,
        track: i32,
        target: i32,
        slot: i32,
        address: u64,
        interpolation: i32,
        row: f64,
        value: f64,
    ) {
        self.control().0.add_automation_point(pattern, track, target, slot, address, interpolation, row, value);
    }

    pub fn add_host_automation_point(
        &mut self,
        pattern: i32,
        destination: i32,
        address: u64,
        interpolation: i32,
        row: f64,
        value: f64,
    ) {
        self.control().0.add_host_automation_point(pattern, destination, address, interpolation, row, value);
    }

    // ---- samples, zones and timeline audio ----

    /// `set_sample(e, instrument, left, right, frames, rate)` with PCM.
    pub fn set_sample(&mut self, instrument: i32, left: &[f32], right: Option<&[f32]>, rate: f64) -> bool {
        let (c, sh, _) = self.control();
        c.set_sample(sh, instrument, Some(Pcm { left, right, rate }))
    }

    /// `set_sample(e, instrument, nullptr, nullptr, 0, rate)`: clears the sample.
    pub fn clear_sample(&mut self, instrument: i32) -> bool {
        let (c, sh, _) = self.control();
        c.set_sample(sh, instrument, None)
    }

    pub fn set_slice_points(&mut self, instrument: i32, positions: &[f32]) {
        self.control().0.set_slice_points(instrument, positions);
    }

    pub fn set_sample_cycle(&mut self, instrument: i32, frames: i32) -> bool {
        let (c, sh, _) = self.control();
        c.set_sample_cycle(sh, instrument, frames)
    }

    pub fn clear_zones(&mut self, instrument: i32) {
        self.control().0.clear_zones(instrument);
    }

    /// `set_audio_asset` with PCM.
    pub fn set_audio_asset(&mut self, asset: i32, left: &[f32], right: Option<&[f32]>, rate: f64) -> bool {
        let (c, sh, _) = self.control();
        c.set_audio_asset(sh, asset, Some(Pcm { left, right, rate }))
    }

    /// `set_audio_asset(e, asset, nullptr, nullptr, 0, rate)`: clears the slot.
    pub fn clear_audio_asset(&mut self, asset: i32) -> bool {
        let (c, sh, _) = self.control();
        c.set_audio_asset(sh, asset, None)
    }

    pub fn clear_audio_clips(&mut self) {
        self.control().0.clear_audio_clips();
    }

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
        self.control().0.add_audio_clip(
            asset,
            track,
            start_beat,
            duration_beats,
            source_offset,
            gain,
            fade_in,
            fade_out,
            muted,
        );
    }

    pub fn add_pattern_clip(&mut self, head_asset: i32, loop_asset: i32, track: i32, pattern: i32, gain: f32) {
        self.control().0.add_pattern_clip(head_asset, loop_asset, track, pattern, gain);
    }

    pub fn add_song_tail_clip(&mut self, asset: i32, track: i32, start_frame: i64, gain: f32) {
        self.control().0.add_song_tail_clip(asset, track, start_frame, gain);
    }

    /// `duration(e, songMode)`
    pub fn duration(&mut self, song_mode: bool) -> f64 {
        self.control().0.duration(song_mode)
    }

    // ---- commands ----

    pub fn play(&mut self, song_mode: i32) {
        let (c, sh, _) = self.control();
        c.play(sh, song_mode);
    }

    pub fn stop(&mut self) {
        let (c, sh, _) = self.control();
        c.stop(sh);
    }

    pub fn play_count_in(&mut self, song_mode: bool, bars: i32) {
        let (c, sh, _) = self.control();
        c.play_count_in(sh, song_mode, bars);
    }

    pub fn set_recording(&mut self, enabled: bool) {
        let (c, sh, _) = self.control();
        c.set_recording(sh, enabled);
    }

    pub fn seek(&mut self, pattern: i32, row: i32, sequence: i32) {
        let (c, sh, _) = self.control();
        c.seek(sh, pattern, row, sequence);
    }

    pub fn set_pattern_loop(&mut self, start_row: i32, end_row: i32) {
        let (c, sh, _) = self.control();
        c.set_pattern_loop(sh, start_row, end_row);
    }

    pub fn queue_jump(&mut self, pattern: i32, sequence: i32) {
        let (c, sh, _) = self.control();
        c.queue_jump(sh, pattern, sequence);
    }

    pub fn queue_mode(&mut self, song_mode: bool, pattern: i32, sequence: i32) {
        let (c, sh, _) = self.control();
        c.queue_mode(sh, song_mode, pattern, sequence);
    }

    pub fn note_on(&mut self, track: i32, instrument: i32, note: i32, velocity: i32) {
        let (c, sh, _) = self.control();
        c.note_on(sh, track, instrument, note, velocity);
    }

    pub fn note_off(&mut self, track: i32, note: i32) {
        let (c, sh, _) = self.control();
        c.note_off(sh, track, note);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn input_note(
        &mut self,
        request_id: u64,
        edge: i32,
        track: i32,
        instrument: i32,
        note: i32,
        velocity: i32,
        apply_note: i32,
    ) -> bool {
        let (c, sh, _) = self.control();
        c.input_note(sh, request_id, edge, track, instrument, note, velocity, apply_note)
    }

    pub fn pop_input_ack(&mut self) -> Option<STInputAck> {
        self.e.control().pop_input_ack()
    }

    /// `preview_step(e, track, step, chord, count, seconds)`
    pub fn preview_step(&mut self, track: i32, step: STStep, chord: &[i32], duration_seconds: f32) {
        let (c, sh, _) = self.control();
        c.preview_step(sh, track, step, chord_list(chord), duration_seconds);
    }

    pub fn panic(&mut self) {
        let (c, sh, _) = self.control();
        c.panic(sh);
    }

    pub fn pitch_bend(&mut self, track: i32, semitones: f32) {
        let (c, sh, _) = self.control();
        c.pitch_bend(sh, track, semitones);
    }

    pub fn mod_wheel(&mut self, track: i32, value: f32) {
        let (c, sh, _) = self.control();
        c.mod_wheel(sh, track, value);
    }

    pub fn set_instrument_parameter(&mut self, instrument: i32, target: i32, value: f32) -> bool {
        let (c, sh, _) = self.control();
        c.set_instrument_parameter(sh, instrument, target, value)
    }

    pub fn refresh_automation(&self) {
        self.e.status().refresh_automation();
    }

    // ---- render side ----

    /// A direct stereo render of `left.len()` frames.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        self.block = None;
        self.e.render().render(left, right);
    }

    /// Advance by `frames` frames without output buffers.
    pub fn render_null(&mut self, frames: u32) {
        self.block = None;
        self.e.render().advance(frames);
    }

    /// One bus of the block at a graph time, `left.len()` frames (see `pull_block`).
    pub fn render_bus(&mut self, bus: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        pull_block(self.e.render(), &mut self.block, time, left.len());
        copy_bus(self.e.render(), bus, left, right);
    }

    /// The block at a graph time without reading a bus.
    pub fn render_block(&mut self, time: u64, frames: u32) {
        pull_block(self.e.render(), &mut self.block, time, frames as usize);
    }

    /// One track's bus of the block at a graph time, or the return bus for a negative channel.
    pub fn render_channel(&mut self, channel: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        let bus = if channel < 0 { return_bus(self.e.status().status().track_count) } else { channel };
        self.render_bus(bus, time, left, right);
    }

    pub fn pop_event(&mut self) -> Option<STEvent> {
        self.e.render().pop_event()
    }

    // ---- any-thread snapshots ----

    pub fn status(&self) -> STStatus {
        self.e.status().status()
    }

    pub fn watch_voice_positions(&self, instrument: i32) {
        self.e.status().watch_voice_positions(instrument);
    }

    /// The voice count (0 when no stable snapshot could be read) and the snapshot serial.
    pub fn voice_positions(&self, out: &mut [STVoicePosition]) -> (usize, u64) {
        self.e.status().voice_positions(out).unwrap_or((0, 0))
    }
}

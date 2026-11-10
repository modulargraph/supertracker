//! An owned engine for tests: one method per `st_engine_*` function of the retired C API, each a
//! plain forward to the safe Rust method that the C function wrapped (`Control` in `edits.rs` /
//! `control.rs`, `Render`, `Shared`). The test owns the engine exclusively, so both thread-owned
//! halves are reached through `UnsafeCell::get_mut`: no `unsafe`, no C pointers.
//!
//! The few argument rules that lived in `capi.rs` rather than in the safe methods are repeated
//! here and marked `TODO(api)`, so the tests keep exercising the behaviour the C++ programs saw.

use tracker_engine::engine::control::Control;
use tracker_engine::engine::edits::Pcm;
use tracker_engine::engine::state::{Render, Shared};
use tracker_engine::engine::tables::CACHE_CAPACITY;
use tracker_engine::engine::*;

pub struct Engine {
    pub e: Box<STEngine>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// A chord as the setters take it: `None` unless at least one note was passed; at most
/// `ST_MAX_CHORD` notes are read (`chord_list` in `capi.rs`).
fn chord_list(chord: &[i32]) -> Option<&[i32]> {
    let notes = &chord[..chord.len().min(ST_MAX_CHORD)];
    (!notes.is_empty()).then_some(notes)
}

fn silence(l: Option<&mut [f32]>, r: Option<&mut [f32]>) {
    for out in [l, r].into_iter().flatten() {
        out.fill(0.0);
    }
}

impl Engine {
    /// `st_engine_create(48000)`
    pub fn new() -> Engine {
        Engine::with_rate(48000.0)
    }

    /// `st_engine_create(rate)`; the C++ fixtures assert the handle.
    pub fn with_rate(rate: f64) -> Engine {
        Engine { e: STEngine::create(rate).expect("st_engine_create") }
    }

    /// The control half, the shared state and the engine's sample rate.
    pub fn control(&mut self) -> (&mut Control, &Shared, f64) {
        let STEngine { sample_rate, shared, control, .. } = &mut *self.e;
        (control.get_mut(), shared, *sample_rate)
    }

    /// The render half and the shared state.
    pub fn renderer(&mut self) -> (&mut Render, &Shared) {
        let STEngine { shared, render, .. } = &mut *self.e;
        (render.get_mut(), shared)
    }

    pub fn shared(&self) -> &Shared {
        &self.e.shared
    }

    pub fn sample_rate(&self) -> f64 {
        self.e.sample_rate
    }

    // ---- update transactions ----

    pub fn begin_update(&mut self) {
        let (c, sh, _) = self.control();
        c.begin_update(sh);
    }

    pub fn clear_project(&mut self) {
        self.control().0.clear_project();
    }

    /// `st_engine_commit_update`: a failed commit keeps the previous project (and the draft).
    pub fn commit_update(&mut self) {
        let _ = self.commit_update_checked();
    }

    /// `st_engine_commit_update_checked`: the error text instead of `0` + buffer.
    pub fn commit_update_checked(&mut self) -> Result<(), String> {
        let (c, sh, rate) = self.control();
        c.commit(sh, rate)
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

    pub fn set_frozen_song_policy(&mut self, enabled: i32, publication_id: u64) -> Result<(), String> {
        let (c, sh, _) = self.control();
        c.set_frozen_song_policy(sh, enabled, publication_id)
    }

    pub fn set_frozen_song_extent(&mut self, publication_id: u64, musical_frames: i64) -> Result<(), String> {
        self.control().0.set_frozen_song_extent(publication_id, musical_frames)
    }

    pub fn set_timing_map(
        &mut self,
        tempos: &[STClockTempoEvent],
        meters: &[STClockMeterEvent],
        revision: u64,
    ) -> Result<(), String> {
        let (c, _, rate) = self.control();
        c.set_timing_map(rate, tempos, meters, revision)
    }

    pub fn clear_timing_map(&mut self) {
        self.control().0.clear_timing_map();
    }

    pub fn set_pattern_timing_origin(&mut self, beat: f64) -> bool {
        self.control().0.set_pattern_timing_origin(beat)
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
        self.control().0.set_host_mixing(enabled, computational_mask);
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

    /// `st_engine_set_step(e, pattern, track, row, step, chord, count)`: an empty `chord` is the
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

    /// `st_engine_set_sample(e, instrument, left, right, frames, rate)` with PCM.
    pub fn set_sample(&mut self, instrument: i32, left: &[f32], right: Option<&[f32]>, rate: f64) -> bool {
        let (c, sh, engine_rate) = self.control();
        c.set_sample(sh, engine_rate, instrument, Some(Pcm { left, right, rate }))
    }

    /// `st_engine_set_sample(e, instrument, nullptr, nullptr, 0, rate)`: clears the sample.
    pub fn clear_sample(&mut self, instrument: i32) -> bool {
        let (c, sh, engine_rate) = self.control();
        c.set_sample(sh, engine_rate, instrument, None)
    }

    pub fn set_slice_points(&mut self, instrument: i32, positions: &[f32]) {
        self.control().0.set_slice_points(instrument, &positions[..positions.len().min(128)]);
    }

    pub fn set_sample_cycle(&mut self, instrument: i32, frames: i32) -> bool {
        let (c, sh, rate) = self.control();
        c.set_sample_cycle(sh, rate, instrument, frames)
    }

    pub fn clear_zones(&mut self, instrument: i32) {
        self.control().0.clear_zones(instrument);
    }

    /// `st_engine_set_audio_asset` with PCM.
    pub fn set_audio_asset(&mut self, asset: i32, left: &[f32], right: Option<&[f32]>, rate: f64) -> bool {
        let (c, sh, engine_rate) = self.control();
        c.set_audio_asset(sh, engine_rate, asset, Some(Pcm { left, right, rate }))
    }

    /// `st_engine_set_audio_asset(e, asset, nullptr, nullptr, 0, rate)`: clears the slot.
    pub fn clear_audio_asset(&mut self, asset: i32) -> bool {
        let (c, sh, engine_rate) = self.control();
        c.set_audio_asset(sh, engine_rate, asset, None)
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

    /// `st_engine_duration(e, songMode)`
    pub fn duration(&mut self, song_mode: bool) -> f64 {
        let (c, _, rate) = self.control();
        c.duration(rate, song_mode)
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

    /// `st_engine_pop_input_ack`
    pub fn pop_input_ack(&mut self) -> Option<STInputAck> {
        self.e.shared.pop_input_ack()
    }

    /// `st_engine_preview_step(e, track, step, chord, count, seconds)`
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
        self.e.shared.refresh_automation();
    }

    // ---- render side ----

    /// `st_engine_render(e, left, right, frames)` with `frames == left.len()`.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        let frames = left.len() as u32;
        let (r, sh) = self.renderer();
        r.render(sh, Some(left), Some(right), frames);
    }

    /// `st_engine_render(e, nullptr, nullptr, frames)`: advance without output buffers.
    pub fn render_null(&mut self, frames: u32) {
        let (r, sh) = self.renderer();
        r.render(sh, None, None, frames);
    }

    /// `render_cached` of `capi.rs`: the argument rules of `st_engine_render_bus` /
    /// `st_engine_render_channel`.
    ///
    /// TODO(api): `Render::render_cached` documents that "the caller has already rejected
    /// `n > CACHE_CAPACITY`, a negative bus and timestamp overflow"; that rejection (silence the
    /// outputs, render nothing) and the zero-frame early return exist only in `capi.rs`.
    fn render_cached(
        &mut self,
        bus: i32,
        channel: bool,
        time: u64,
        left: Option<&mut [f32]>,
        right: Option<&mut [f32]>,
        frames: u32,
    ) {
        if (!channel && bus < 0) || frames as usize > CACHE_CAPACITY || time > u64::MAX - frames as u64 {
            silence(left, right);
            return;
        }
        if frames != 0 {
            let (r, sh) = self.renderer();
            r.render_cached(sh, bus, channel, time, left, right, frames);
        }
    }

    /// `st_engine_render_bus(e, bus, time, left, right, frames)` with `frames == left.len()`.
    pub fn render_bus(&mut self, bus: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        let frames = left.len() as u32;
        self.render_cached(bus, false, time, Some(left), Some(right), frames);
    }

    /// `st_engine_render_bus(e, bus, time, nullptr, nullptr, frames)`
    pub fn render_bus_null(&mut self, bus: i32, time: u64, frames: u32) {
        self.render_cached(bus, false, time, None, None, frames);
    }

    /// `st_engine_render_channel(e, channel, time, left, right, frames)`
    pub fn render_channel(&mut self, channel: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        let frames = left.len() as u32;
        self.render_cached(channel, true, time, Some(left), Some(right), frames);
    }

    /// `st_engine_render_channel(e, channel, time, nullptr, nullptr, frames)`
    pub fn render_channel_null(&mut self, channel: i32, time: u64, frames: u32) {
        self.render_cached(channel, true, time, None, None, frames);
    }

    pub fn reset_render_cache(&self) {
        self.e.shared.reset_render_cache();
    }

    /// `st_engine_clock_span`: the status; `out` is written only for `ST_CLOCK_OK`.
    pub fn clock_span(&mut self, graph_frame: i64, requested: u32, latency: i64, out: &mut STClockSpan) -> i32 {
        let (r, sh) = self.renderer();
        r.clock_span(sh, graph_frame, requested, latency, out)
    }

    /// `st_engine_freeze_span`: the status; `out` is written only for `ST_CLOCK_OK`.
    pub fn freeze_span(&mut self, graph_frame: i64, requested: u32, latency: i64, out: &mut STFreezeSpan) -> i32 {
        let (r, sh) = self.renderer();
        r.freeze_span(sh, graph_frame, requested, latency, out)
    }

    pub fn capture_transport(&mut self) -> STTransportState {
        let (r, sh) = self.renderer();
        r.capture_transport(sh)
    }

    pub fn restore_transport(&mut self, state: STTransportState, resume: bool) {
        let (r, sh) = self.renderer();
        r.restore_transport(sh, &state, resume);
    }

    pub fn forget_external_voices(&mut self, channel: i32) -> i32 {
        let (r, sh) = self.renderer();
        r.forget_external_voices(sh, channel)
    }

    /// `st_engine_pop_event`
    pub fn pop_event(&mut self) -> Option<STEvent> {
        self.e.shared.events.pop()
    }

    // ---- any-thread snapshots ----

    pub fn status(&self) -> STStatus {
        self.e.shared.status()
    }

    pub fn track_peak(&self, track: i32) -> f32 {
        self.e.shared.track_peak(track)
    }

    pub fn watch_voice_positions(&self, instrument: i32) {
        self.e.shared.watch_voice_positions(instrument);
    }

    /// `st_engine_voice_positions(e, voices, capacity, &serial)`: the voice count (0 when no
    /// stable snapshot could be read) and the snapshot serial.
    pub fn voice_positions(&self, out: &mut [STVoicePosition]) -> (usize, u64) {
        let capacity = out.len().min(ST_VOICE_POSITION_CAPACITY);
        self.e.shared.voice_positions(&mut out[..capacity]).unwrap_or((0, 0))
    }
}

/// `st_instrument_capacity`: the native instrument table holds exactly the project's instruments.
///
/// TODO(api): the header's inline `st_instrument_capacity(count)` has no Rust counterpart.
pub fn st_instrument_capacity(instrument_count: i32) -> i32 {
    instrument_count
}

/// `st_channel_output_bus`: a channel's bus has the channel's own number.
///
/// TODO(api): the header's inline `st_channel_output_bus(trackCount, channel)` has no Rust
/// counterpart.
pub fn st_channel_output_bus(_track_count: i32, channel: i32) -> i32 {
    channel
}

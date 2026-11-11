//! An engine for tests: one method per operation, each a plain forward to the engine's API
//! (the `Control` methods reached through the control half, and the render half's own methods).
//!
//! A test engine is a pair of engines that receive the same edits and commands. One runs under
//! host mixing, as the mixer crate runs it, and is rendered block by block; the other renders
//! the engine's own stereo mix. A test reads one of the two, by its view ([`both_views`]);
//! every event, status and result the two engines give is compared.

use std::cell::Cell;
use tracker_engine::engine::control::Control;
use tracker_engine::engine::edits::Pcm;
use tracker_engine::engine::host;
use tracker_engine::engine::state::Shared;
use tracker_engine::engine::tables::MAX_BLOCK;
use tracker_engine::engine::*;

thread_local! {
    /// Which output the engines made on this thread hand to the test: the engine's own mix
    /// (`Some(true)`) or the buses under host mixing (`Some(false)`).
    static MIX_VIEW: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Runs a test twice: once reading the engine's own stereo mix (the output the expectations
/// were first recorded from) and once reading the buses under host mixing (what a host
/// consumes). The same assertions hold for both unless the test asks [`mix_view`].
pub fn both_views(test: impl Fn()) {
    for mix in [true, false] {
        MIX_VIEW.with(|view| view.set(Some(mix)));
        test();
    }
    MIX_VIEW.with(|view| view.set(None));
}

/// Runs a test whose subject is the engine's own mix, reading that mix only.
pub fn mix_view_only(test: impl FnOnce()) {
    MIX_VIEW.with(|view| view.set(Some(true)));
    test();
    MIX_VIEW.with(|view| view.set(None));
}

/// Runs a test that reads the buses under host mixing only.
pub fn host_view_only(test: impl FnOnce()) {
    MIX_VIEW.with(|view| view.set(Some(false)));
    test();
    MIX_VIEW.with(|view| view.set(None));
}

/// Whether the running test reads the engine's own mix (see [`both_views`]).
pub fn mix_view() -> bool {
    MIX_VIEW.with(Cell::get).expect("a test runs inside both_views, mix_view_only or host_view_only")
}

/// The longest render of one call.
const UNREAD_FRAMES: usize = 1 << 21;

/// One engine of the pair.
struct One {
    e: host::Engine,
    /// Host mixing is kept on through every project reset.
    host_mixing: bool,
    /// See [`pull_block`].
    block: Option<(u64, usize)>,
}

pub struct Engine {
    host: Option<One>,
    mix: Option<One>,
    mix_view: bool,
    /// The host's frame count: where the next block of a plain render starts.
    clock: u64,
    /// Where the engine the test does not read renders: allocated with the engine, so that a
    /// render allocates nothing.
    unread: (Vec<f32>, Vec<f32>),
}

/// The buses of an engine as the tests pull them: one bus of a block at a time.
///
/// `block` is the range (start, frames) rendered last. A pull of that range reads the block
/// again (the tests take one bus after another); any other pull renders the next block, which
/// must follow the previous one and be at most `MAX_BLOCK` frames long. `None` after a plain
/// render: the next block may start at any time.
fn pull_block(render: &mut host::RenderHalf, block: &mut Option<(u64, usize)>, time: u64, frames: usize) {
    if *block == Some((time, frames)) {
        return;
    }
    assert!((1..=MAX_BLOCK).contains(&frames), "a pull is one block of 1..=MAX_BLOCK frames");
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

/// Renders `left.len()` frames from `clock` on, block by block as a host does, and writes the
/// sum of every bus: each track's bus in track order, then the return bus. Nothing else is
/// applied. Events stay queued; their frame offsets count from the start of their own block.
fn render_bus_sum(render: &mut host::RenderHalf, clock: &mut u64, left: &mut [f32], right: &mut [f32]) {
    assert_eq!(left.len(), right.len());
    let mut at = 0;
    while at < left.len() {
        let count = MAX_BLOCK.min(left.len() - at);
        let (l, r) = (&mut left[at..at + count], &mut right[at..at + count]);
        assert_eq!(render.render_block(*clock, count as u32) as usize, count);
        let view = render.block();
        l.fill(0.0);
        r.fill(0.0);
        let tracks = (0..view.track_count()).filter_map(|track| view.track(track));
        for (bus_l, bus_r) in tracks.chain(std::iter::once(view.return_bus())) {
            for (out, sample) in l.iter_mut().zip(bus_l) {
                *out += *sample;
            }
            for (out, sample) in r.iter_mut().zip(bus_r) {
                *out += *sample;
            }
        }
        *clock += count as u64;
        at += count;
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

/// What the two engines of a pair answered: the same thing. With one engine, its answer.
#[track_caller]
fn agreed<T: PartialEq + std::fmt::Debug>(host: Option<T>, mix: Option<T>) -> T {
    match (host, mix) {
        (Some(host), Some(mix)) => {
            assert_eq!(mix, host, "the mix engine (left) and the engine under host mixing (right) differ");
            host
        }
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => unreachable!("a test engine has at least one engine"),
    }
}

impl One {
    /// An engine at `rate`, with host mixing on as the mixer crate sets one up, or with the
    /// engine's own mix.
    fn new(rate: f64, host_mixing: bool) -> One {
        let mut e = host::Engine::new(rate).expect("an engine at a supported rate");
        if host_mixing {
            let (control, shared, _) = e.control().parts();
            control.begin_update(shared);
            control.set_host_mixing_tracks(true, &[], true);
            control.commit(shared).expect("an empty project is accepted");
        }
        One { e, host_mixing, block: None }
    }

    /// The control state, the shared state its methods take and the engine's sample rate.
    fn control(&mut self) -> (&mut Control, &Shared, f64) {
        self.e.control().parts()
    }
}

impl Engine {
    /// An engine at 48 kHz.
    pub fn new() -> Engine {
        Engine::with_rate(48000.0)
    }

    fn with(host: Option<One>, mix: Option<One>) -> Engine {
        Engine { host, mix, mix_view: mix_view(), clock: 0, unread: (vec![0.0; UNREAD_FRAMES], vec![0.0; UNREAD_FRAMES]) }
    }

    /// An engine at `rate`.
    pub fn with_rate(rate: f64) -> Engine {
        Engine::with(Some(One::new(rate, true)), Some(One::new(rate, false)))
    }

    /// Only the engine the test's view reads, at 48 kHz: for tests that time a render, or whose
    /// two engines legitimately differ in more than their audio.
    pub fn alone() -> Engine {
        if mix_view() {
            Engine::with(None, Some(One::new(48000.0, false)))
        } else {
            Engine::with(Some(One::new(48000.0, true)), None)
        }
    }

    /// The engine the test reads.
    fn read(&self) -> &One {
        let (first, second) = if self.mix_view { (&self.mix, &self.host) } else { (&self.host, &self.mix) };
        first.as_ref().or(second.as_ref()).expect("a test engine has at least one engine")
    }

    pub fn sample_rate(&self) -> f64 {
        self.read().e.sample_rate()
    }
}

/// Defines each control method for one engine and forwards it from [`Engine`] to both engines
/// of the pair, comparing what they return.
macro_rules! pair_methods {
    ($( $(#[$meta:meta])* pub fn $name:ident(&mut $this:ident $(, $argument:ident : $kind:ty)* $(,)?) $(-> $result:ty)? $body:block )*) => {
        impl One {
            $( #[allow(clippy::too_many_arguments)] fn $name(&mut $this $(, $argument: $kind)*) $(-> $result)? $body )*
        }
        impl Engine {
            $( $(#[$meta])* pub fn $name(&mut self $(, $argument: $kind)*) $(-> $result)? {
                let mix = self.mix.as_mut().map(|mix| mix.$name($($argument),*));
                let host = self.host.as_mut().map(|host| host.$name($($argument),*));
                agreed(host, mix)
            } )*
        }
    };
}

pair_methods! {
    // ---- update transactions ----

    pub fn begin_update(&mut self) {
        let (c, sh, _) = self.control();
        c.begin_update(sh);
    }

    pub fn clear_project(&mut self) {
        self.control().0.clear_project();
        if self.host_mixing {
            self.control().0.set_host_mixing_tracks(true, &[], true);
        }
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

    /// `set_step(e, pattern, track, row, step, chord, count)`: an empty `chord` is the
    /// C call's `nullptr, 0`.
    pub fn set_step(&mut self, pattern: i32, track: i32, row: i32, step: STStep, chord: &[i32]) {
        self.control().0.set_step(pattern, track, row, step, chord_list(chord));
    }

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

}

impl Engine {
    /// The pattern epoch of the engine the test reads (each engine has its own).
    pub fn pattern_epoch(&mut self) -> u64 {
        let mix = self.mix.as_mut().map(|mix| mix.control().0.pattern_epoch());
        let host = self.host.as_mut().map(|host| host.control().0.pattern_epoch());
        if self.mix_view { mix.or(host) } else { host.or(mix) }.expect("an engine")
    }

    /// The next input acknowledgement of the engine the test reads. Both engines of a pair
    /// acknowledge alike, except for the frame counts: `graph_frame` exists for a block render
    /// only, and `render_frame` counts each engine's own renders.
    pub fn pop_input_ack(&mut self) -> Option<STInputAck> {
        let mix = self.mix.as_mut().map(|mix| mix.e.control().pop_input_ack());
        let host = self.host.as_mut().map(|host| host.e.control().pop_input_ack());
        if let (Some(mix), Some(host)) = (mix, host) {
            let same = match (mix, host) {
                (Some(a), Some(b)) => STInputAck { graph_frame: b.graph_frame, render_frame: b.render_frame, ..a } == b,
                (None, None) => true,
                _ => false,
            };
            assert!(same, "the engines' input acknowledgements differ: {mix:?} from the mix engine, {host:?} under host mixing");
        }
        if self.mix_view { mix.or(host) } else { host.or(mix) }.flatten()
    }

    pub fn refresh_automation(&self) {
        for one in self.mix.iter().chain(self.host.iter()) {
            one.e.status().refresh_automation();
        }
    }

    // ---- render side ----

    /// A render of `left.len()` frames. The mix engine renders its own stereo mix in one call;
    /// the engine under host mixing renders block by block and gives the sum of its buses (see
    /// `render_bus_sum`). The buffers receive the output of the test's view.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        let unread = (&mut self.unread.0[..left.len()], &mut self.unread.1[..left.len()]);
        if let Some(mix) = self.mix.as_mut() {
            let (l, r) = if self.mix_view { (&mut *left, &mut *right) } else { (&mut *unread.0, &mut *unread.1) };
            mix.block = None;
            mix.e.render().render(l, r);
        }
        if let Some(host) = self.host.as_mut() {
            let (l, r) = if self.mix_view { (&mut *unread.0, &mut *unread.1) } else { (&mut *left, &mut *right) };
            host.block = None;
            render_bus_sum(host.e.render(), &mut self.clock, l, r);
        }
    }

    /// Advance by `frames` frames without reading the output.
    pub fn render_null(&mut self, frames: u32) {
        if let Some(mix) = self.mix.as_mut() {
            mix.block = None;
            mix.e.render().advance(frames);
        }
        if let Some(host) = self.host.as_mut() {
            host.block = None;
            let mut left = frames as usize;
            while left > 0 {
                let count = MAX_BLOCK.min(left);
                assert_eq!(host.e.render().render_block(self.clock, count as u32) as usize, count);
                self.clock += count as u64;
                left -= count;
            }
        }
    }

    /// One bus of the block at a graph time, `left.len()` frames (see `pull_block`): both
    /// engines render the block, the buffers receive the bus of the test's view.
    pub fn render_bus(&mut self, bus: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        let unread = (&mut self.unread.0[..left.len()], &mut self.unread.1[..left.len()]);
        if let Some(mix) = self.mix.as_mut() {
            let (l, r) = if self.mix_view { (&mut *left, &mut *right) } else { (&mut *unread.0, &mut *unread.1) };
            pull_block(mix.e.render(), &mut mix.block, time, l.len());
            copy_bus(mix.e.render(), bus, l, r);
        }
        if let Some(host) = self.host.as_mut() {
            let (l, r) = if self.mix_view { (&mut *unread.0, &mut *unread.1) } else { (&mut *left, &mut *right) };
            pull_block(host.e.render(), &mut host.block, time, l.len());
            copy_bus(host.e.render(), bus, l, r);
            self.clock = time + left.len() as u64;
        }
    }

    /// The block at a graph time without reading a bus.
    pub fn render_block(&mut self, time: u64, frames: u32) {
        for one in self.mix.iter_mut().chain(self.host.iter_mut()) {
            pull_block(one.e.render(), &mut one.block, time, frames as usize);
        }
        self.clock = time + frames as u64;
    }

    /// See `RenderHalf::block_song_frame`.
    pub fn block_song_frame(&mut self) -> Option<i64> {
        let mix = self.mix.as_mut().map(|mix| mix.e.render().block_song_frame());
        let host = self.host.as_mut().map(|host| host.e.render().block_song_frame());
        agreed(host, mix)
    }

    /// One track's bus of the block at a graph time, or the return bus for a negative channel.
    pub fn render_channel(&mut self, channel: i32, time: u64, left: &mut [f32], right: &mut [f32]) {
        let bus = if channel < 0 { return_bus(self.read().e.status().status().track_count) } else { channel };
        self.render_bus(bus, time, left, right);
    }

    /// The next event. Both engines of a pair must have the same event next. The frame offset
    /// of the mix engine's event counts from the start of its one render call, which is a whole
    /// number of blocks before the start of the block the event has under host mixing.
    pub fn pop_event(&mut self) -> Option<STEvent> {
        let mix = self.mix.as_mut().map(|mix| mix.e.render().pop_event());
        let host = self.host.as_mut().map(|host| host.e.render().pop_event());
        if let (Some(mix), Some(host)) = (mix, host) {
            let same = match (mix, host) {
                (Some(a), Some(b)) => {
                    let blocks = a.frame_offset - b.frame_offset;
                    blocks >= 0 && blocks % MAX_BLOCK as i32 == 0 && STEvent { frame_offset: b.frame_offset, ..a } == b
                }
                (None, None) => true,
                _ => false,
            };
            assert!(same, "the engines' events differ: {mix:?} from the mix engine, {host:?} under host mixing");
        }
        if self.mix_view { mix.or(host) } else { host.or(mix) }.flatten()
    }

    // ---- any-thread snapshots ----

    pub fn status(&self) -> STStatus {
        agreed(self.host.as_ref().map(|host| host.e.status().status()), self.mix.as_ref().map(|mix| mix.e.status().status()))
    }

    pub fn watch_voice_positions(&self, instrument: i32) {
        for one in self.mix.iter().chain(self.host.iter()) {
            one.e.status().watch_voice_positions(instrument);
        }
    }

    /// The voice count (0 when no stable snapshot could be read) and the snapshot serial, of
    /// the engine the test reads.
    pub fn voice_positions(&self, out: &mut [STVoicePosition]) -> (usize, u64) {
        self.read().e.status().voice_positions(out).unwrap_or((0, 0))
    }
}

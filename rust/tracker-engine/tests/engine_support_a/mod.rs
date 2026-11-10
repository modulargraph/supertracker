//! Shared helpers for the engine integration tests `tests/engine_core_*.rs` and
//! `tests/engine_timing_*.rs`: ports of the C++ programs `tests/engine_tests.cpp` and
//! `tests/engine_timing_tests.cpp`.
//!
//! * [`Engine`]: an owned engine whose methods carry the names and argument order of the
//!   `st_engine_*` functions the C++ tests called, implemented on the engine's safe Rust API
//!   (`Control` / `Render` / `Shared` methods). No `extern "C"` function is used.
//! * [`realtime`] and [`assert_zero_render_allocations`]: the C++ tests' replaced
//!   `operator new` with its thread-local "render in progress" flag.
//! * [`tone`]: `tests/engine_test_tone.hpp`.
//! * [`core_helpers`], [`timing_helpers`]: the fixtures and helpers of the two C++ programs.
#![allow(dead_code)]

pub mod core_helpers;
pub mod timing_helpers;
pub mod tone;

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use tracker_engine::clock::{STClockMeterEvent, STClockSpan, STClockTempoEvent};
use tracker_engine::engine::control::Control;
use tracker_engine::engine::edits::Pcm;
use tracker_engine::engine::state::{Render, Shared};
use tracker_engine::engine::tables::CACHE_CAPACITY;
use tracker_engine::engine::{
    STEngine, STEvent, STInstrument, STStatus, STStep, STTrack, STTransportState,
};

// ---------------------------------------------------------------------------------------------
// Allocation counting: the C++ tests' `operator new` with `thread_local bool realtime`.
// ---------------------------------------------------------------------------------------------

thread_local! {
    /// C++ `static thread_local bool realtime`.
    static REALTIME: Cell<bool> = const { Cell::new(false) };
}

/// C++ `static std::atomic<unsigned> realtimeAllocations`: allocations made by any thread while
/// its render-in-progress flag was set.
static REALTIME_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

#[cfg(not(feature = "cxx-alloc"))]
mod allocator {
    use super::{REALTIME, REALTIME_ALLOCATIONS};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::Ordering;

    pub struct CountingAllocator;

    #[inline]
    fn note() {
        // `try_with`: the allocator also runs while a thread's locals are being torn down.
        if REALTIME.try_with(Cell::get).unwrap_or(false) {
            REALTIME_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }

    // SAFETY: defers to the system allocator; the bookkeeping never allocates.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note();
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            note();
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            note();
            unsafe { System.realloc(pointer, layout, size) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
}

/// Run `f` as the C++ tests ran an engine call between `realtime=true` and `realtime=false`:
/// every heap allocation the calling thread makes meanwhile is counted.
#[inline]
pub fn realtime<T>(f: impl FnOnce() -> T) -> T {
    let before = REALTIME.with(|flag| flag.replace(true));
    let result = f();
    REALTIME.with(|flag| flag.set(before));
    result
}

/// Allocations counted inside [`realtime`] sections so far, on every thread of this test binary.
pub fn realtime_allocations() -> u64 {
    REALTIME_ALLOCATIONS.load(Ordering::Relaxed)
}

/// C++ `assert(realtimeAllocations.load()==0)` at the end of `main`. Every `#[test]` ends with
/// it, so each suite fails on its own when a render-thread call allocated (the counter is
/// process-wide, like the C++ one: once a render allocated, every later check fails too).
#[track_caller]
pub fn assert_zero_render_allocations() {
    assert_eq!(realtime_allocations(), 0, "a render-thread call allocated");
}

/// Sets the flag when dropped. The multi-thread tests end their loops on such flags; setting
/// them from a guard ends the other threads even when the thread that owns the guard fails an
/// assertion, so a failing test reports instead of hanging.
pub struct SetOnDrop<'a>(pub &'a std::sync::atomic::AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------------

/// C++ `struct Engine`: an engine at 48 kHz that is destroyed with the test.
pub struct Engine {
    engine: Box<STEngine>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// The control-thread half of an engine: every "ordinary control function" of the C API.
pub struct ControlHalf<'a> {
    control: &'a mut Control,
    shared: &'a Shared,
    sample_rate: f64,
}

/// The render-thread half of an engine: render, bus pulls, clock queries, event consumption and
/// the exclusive stopped-graph operations.
pub struct RenderHalf<'a> {
    render: &'a mut Render,
    shared: &'a Shared,
}

/// The part of an engine any thread may use: status, peaks and the refresh/reset mailboxes.
#[derive(Clone, Copy)]
pub struct SharedHalf<'a> {
    shared: &'a Shared,
}

// TODO(api): the engine has no way to hand its halves to different threads. `Shared` is not
// `Sync` (the voice-snapshot seqlock is an `UnsafeCell`) and `Control`/`Render` are not `Send`
// (they hold raw project pointers), although the engine's threading contract (one control
// thread, one render thread, any thread for `Shared`) is exactly what these impls state. The C
// API reached the same thing through `*mut STEngine`.
//
// SAFETY: each half is used by one thread at a time (the tests move a half into one scoped
// thread); `Shared` is the engine's lock-free cross-thread state (atomics, SPSC rings, the
// seqlock-guarded voice snapshot) and is designed to be shared between exactly these threads.
unsafe impl Send for ControlHalf<'_> {}
unsafe impl Send for RenderHalf<'_> {}
unsafe impl Send for SharedHalf<'_> {}
unsafe impl Sync for SharedHalf<'_> {}

impl Engine {
    /// C++ `Engine e;` (`st_engine_create(48000)` with `assert(e)`).
    pub fn new() -> Engine {
        Engine::at(48000.0)
    }

    /// `st_engine_create(rate)` with `assert(e)`.
    pub fn at(rate: f64) -> Engine {
        let engine = Engine::create(rate);
        assert!(engine.is_some());
        engine.unwrap()
    }

    /// `st_engine_create(rate)`: `None` where the C function returned a null handle.
    pub fn create(rate: f64) -> Option<Engine> {
        STEngine::create(rate).map(|engine| Engine { engine })
    }

    /// The three thread-owned parts, for tests that drive the engine from several threads.
    pub fn split(&mut self) -> (ControlHalf<'_>, RenderHalf<'_>, SharedHalf<'_>) {
        let STEngine { sample_rate, shared, control, render } = &mut *self.engine;
        let shared: &Shared = shared;
        (
            ControlHalf { control: control.get_mut(), shared, sample_rate: *sample_rate },
            RenderHalf { render: render.get_mut(), shared },
            SharedHalf { shared },
        )
    }

    fn control_half(&mut self) -> ControlHalf<'_> {
        self.split().0
    }

    fn render_half(&mut self) -> RenderHalf<'_> {
        self.split().1
    }

    fn shared_half(&self) -> SharedHalf<'_> {
        SharedHalf { shared: &self.engine.shared }
    }
}

/// Defines each method once on a half and forwards it from [`Engine`].
macro_rules! half_methods {
    ($half:ident, $accessor:ident; $( $(#[$meta:meta])* fn $name:ident(&mut $this:ident $(, $argument:ident : $kind:ty)* $(,)?) $(-> $result:ty)? $body:block )*) => {
        impl $half<'_> {
            $( $(#[$meta])* pub fn $name(&mut $this $(, $argument: $kind)*) $(-> $result)? $body )*
        }
        impl Engine {
            $( $(#[$meta])* pub fn $name(&mut self $(, $argument: $kind)*) $(-> $result)? {
                self.$accessor().$name($($argument),*)
            } )*
        }
    };
}

/// A chord as the setters take it: `None` unless at least one note was passed (C: a null
/// pointer or a count of zero).
fn chord_list(chord: &[i32]) -> Option<&[i32]> {
    (!chord.is_empty()).then_some(chord)
}

half_methods! { ControlHalf, control_half;
    // ---- update transactions ----

    /// `st_engine_begin_update`
    fn begin_update(&mut self) {
        self.control.begin_update(self.shared);
    }
    /// `st_engine_clear_project`
    fn clear_project(&mut self) {
        self.control.clear_project();
    }
    /// `st_engine_commit_update`: a rejected draft stays unpublished, silently.
    fn commit_update(&mut self) {
        let _ = self.control.commit(self.shared, self.sample_rate);
    }
    /// `st_engine_commit_update_checked`: the error is the text the C function wrote into its
    /// error buffer.
    fn commit_update_checked(&mut self) -> Result<(), String> {
        self.control.commit(self.shared, self.sample_rate)
    }
    /// `st_engine_cancel_update`
    fn cancel_update(&mut self) {
        self.control.cancel_update();
    }
    /// `st_engine_collect`
    fn collect(&mut self) {
        self.control.collect(self.shared);
    }

    // ---- project settings ----

    /// `st_engine_set_timing`
    fn set_timing(&mut self, bpm: f64, lines_per_beat: i32, swing: f32, master_volume: f32) {
        self.control.set_timing(bpm, lines_per_beat, swing, master_volume);
    }
    /// `st_engine_set_timing_map`
    fn set_timing_map(&mut self, tempos: &[STClockTempoEvent], meters: &[STClockMeterEvent], revision: u64) -> Result<(), String> {
        self.control.set_timing_map(self.sample_rate, tempos, meters, revision)
    }
    /// `st_engine_clear_timing_map`
    fn clear_timing_map(&mut self) {
        self.control.clear_timing_map();
    }
    /// `st_engine_set_pattern_timing_origin`
    fn set_pattern_timing_origin(&mut self, beat: f64) -> bool {
        self.control.set_pattern_timing_origin(beat)
    }
    /// `st_engine_set_metronome`
    fn set_metronome(&mut self, enabled: i32, numerator: i32, denominator: i32, volume: f32) {
        self.control.set_metronome(enabled != 0, numerator, denominator, volume);
    }
    /// `st_engine_set_track_count`
    fn set_track_count(&mut self, count: i32) -> bool {
        self.control.set_track_count(count)
    }
    /// `st_engine_set_track`
    fn set_track(&mut self, index: i32, track: STTrack) {
        self.control.set_track(index, track);
    }
    /// `st_engine_set_host_mixing`
    fn set_host_mixing(&mut self, enabled: i32, computational_mask: u32) {
        self.control.set_host_mixing(enabled != 0, computational_mask);
    }
    /// `st_engine_set_instrument`
    fn set_instrument(&mut self, index: i32, instrument: STInstrument) {
        self.control.set_instrument(index, instrument);
    }
    /// `st_engine_set_sequence`
    fn set_sequence(&mut self, patterns: &[i32], loop_song: i32, active_pattern: i32) {
        self.control.set_sequence(patterns, loop_song != 0, active_pattern);
    }
    /// `st_engine_set_effects`
    fn set_effects(&mut self, delay_seconds: f32, delay_feedback: f32, delay_mix: f32, reverb_size: f32, reverb_damping: f32, reverb_mix: f32) {
        self.control.set_effects(delay_seconds, delay_feedback, delay_mix, reverb_size, reverb_damping, reverb_mix);
    }
    /// `st_engine_set_host_automation_map`
    fn set_host_automation_map(&mut self, map: u64) {
        self.control.set_host_automation_map(map);
    }

    // ---- patterns, steps and automation ----

    /// `st_engine_set_pattern`
    fn set_pattern(&mut self, index: i32, length: i32) {
        self.control.set_pattern(index, length);
    }
    /// `st_engine_reuse_pattern`
    fn reuse_pattern(&mut self, index: i32) -> bool {
        self.control.reuse_pattern(index)
    }
    /// `st_engine_set_step`; an empty `chord` is the C call's `nullptr, 0`.
    fn set_step(&mut self, pattern: i32, track: i32, row: i32, step: STStep, chord: &[i32]) {
        self.control.set_step(pattern, track, row, step, chord_list(chord));
    }
    /// `st_engine_set_step_column`
    fn set_step_column(&mut self, pattern: i32, track: i32, row: i32, column: i32, step: STStep) {
        self.control.set_step_column(pattern, track, row, column, step);
    }
    /// `st_engine_clear_step`
    fn clear_step(&mut self, pattern: i32, track: i32, row: i32) {
        self.control.clear_step(pattern, track, row);
    }
    /// `st_engine_clear_automation`
    fn clear_automation(&mut self, pattern: i32) {
        self.control.clear_automation(pattern);
    }
    /// `st_engine_add_automation_point`
    fn add_automation_point(&mut self, pattern: i32, track: i32, target: i32, slot: i32, address: u64, interpolation: i32, row: f64, value: f64) {
        self.control.add_automation_point(pattern, track, target, slot, address, interpolation, row, value);
    }
    /// `st_engine_add_host_automation_point`
    fn add_host_automation_point(&mut self, pattern: i32, destination: i32, address: u64, interpolation: i32, row: f64, value: f64) {
        self.control.add_host_automation_point(pattern, destination, address, interpolation, row, value);
    }

    // ---- samples, zones and timeline audio ----

    /// `st_engine_set_sample(e, instrument, left, nullptr, frames, rate)`: mono PCM.
    fn set_sample(&mut self, instrument: i32, left: &[f32], rate: f64) -> bool {
        self.control.set_sample(self.shared, self.sample_rate, instrument, Some(Pcm { left, right: None, rate }))
    }
    /// `st_engine_set_sample(e, instrument, nullptr, nullptr, 0, rate)`: remove the sample.
    fn clear_sample(&mut self, instrument: i32) -> bool {
        self.control.set_sample(self.shared, self.sample_rate, instrument, None)
    }
    /// `st_engine_set_slice_points`
    fn set_slice_points(&mut self, instrument: i32, positions: &[f32]) {
        self.control.set_slice_points(instrument, positions);
    }
    /// `st_engine_clear_zones`
    fn clear_zones(&mut self, instrument: i32) {
        self.control.clear_zones(instrument);
    }
    /// `st_engine_add_sample_zone` with mono PCM.
    fn add_sample_zone(&mut self, instrument: i32, left: &[f32], rate: f64, key_low: i32, key_high: i32, velocity_low: i32, velocity_high: i32, root_note: i32, tune: f32, volume: f32) -> bool {
        self.control.add_sample_zone(
            self.shared,
            self.sample_rate,
            instrument,
            Pcm { left, right: None, rate },
            key_low,
            key_high,
            velocity_low,
            velocity_high,
            root_note,
            tune,
            volume,
        )
    }
    /// `st_engine_set_audio_asset` with mono PCM.
    fn set_audio_asset(&mut self, asset: i32, left: &[f32], rate: f64) -> bool {
        self.control.set_audio_asset(self.shared, self.sample_rate, asset, Some(Pcm { left, right: None, rate }))
    }
    /// `st_engine_clear_audio_clips`
    fn clear_audio_clips(&mut self) {
        self.control.clear_audio_clips();
    }
    /// `st_engine_add_audio_clip`
    fn add_audio_clip(&mut self, asset: i32, track: i32, start_beat: f64, duration_beats: f64, source_offset: f64, gain: f32, fade_in: f32, fade_out: f32, muted: i32) {
        self.control.add_audio_clip(asset, track, start_beat, duration_beats, source_offset, gain, fade_in, fade_out, muted != 0);
    }
    /// `st_engine_duration`
    fn duration(&mut self, song_mode: i32) -> f64 {
        self.control.duration(self.sample_rate, song_mode != 0)
    }

    // ---- commands ----

    /// `st_engine_play`
    fn play(&mut self, song_mode: i32) {
        self.control.play(self.shared, song_mode);
    }
    /// `st_engine_stop`
    fn stop(&mut self) {
        self.control.stop(self.shared);
    }
    /// `st_engine_play_count_in`
    fn play_count_in(&mut self, song_mode: i32, bars: i32) {
        self.control.play_count_in(self.shared, song_mode != 0, bars);
    }
    /// `st_engine_set_recording`
    fn set_recording(&mut self, enabled: i32) {
        self.control.set_recording(self.shared, enabled != 0);
    }
    /// `st_engine_seek`
    fn seek(&mut self, pattern: i32, row: i32, sequence: i32) {
        self.control.seek(self.shared, pattern, row, sequence);
    }
    /// `st_engine_set_pattern_loop`
    fn set_pattern_loop(&mut self, start_row: i32, end_row: i32) {
        self.control.set_pattern_loop(self.shared, start_row, end_row);
    }
    /// `st_engine_queue_jump`
    fn queue_jump(&mut self, pattern: i32, sequence: i32) {
        self.control.queue_jump(self.shared, pattern, sequence);
    }
    /// `st_engine_queue_mode`
    fn queue_mode(&mut self, song_mode: i32, pattern: i32, sequence: i32) {
        self.control.queue_mode(self.shared, song_mode != 0, pattern, sequence);
    }
    /// `st_engine_note_on`
    fn note_on(&mut self, track: i32, instrument: i32, note: i32, velocity: i32) {
        self.control.note_on(self.shared, track, instrument, note, velocity);
    }
    /// `st_engine_note_off`
    fn note_off(&mut self, track: i32, note: i32) {
        self.control.note_off(self.shared, track, note);
    }
    /// `st_engine_preview_step`; an empty `chord` is the C call's null pointer or zero count.
    fn preview_step(&mut self, track: i32, step: STStep, chord: &[i32], duration_seconds: f32) {
        self.control.preview_step(self.shared, track, step, chord_list(chord), duration_seconds);
    }
    /// `st_engine_panic`
    fn panic(&mut self) {
        self.control.panic(self.shared);
    }
    /// `st_engine_pitch_bend`
    fn pitch_bend(&mut self, track: i32, semitones: f32) {
        self.control.pitch_bend(self.shared, track, semitones);
    }
    /// `st_engine_mod_wheel`
    fn mod_wheel(&mut self, track: i32, value: f32) {
        self.control.mod_wheel(self.shared, track, value);
    }
}

half_methods! { RenderHalf, render_half;
    /// `st_engine_render` into two buffers of equal length, counted as a render-thread call.
    fn render_into(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len() as u32;
        realtime(|| self.render.render(self.shared, Some(left), Some(right), frames));
    }
    /// `st_engine_render(e, nullptr, nullptr, 0)`: apply queued commands without audio.
    fn render_nothing(&mut self) {
        realtime(|| self.render.render(self.shared, None, None, 0));
    }
    /// `st_engine_render_bus`, counted as a render-thread call.
    ///
    // TODO(api): the argument checks of `renderCached` (a negative bus, more frames than the
    // bus history holds, a timestamp that overflows, zero frames) exist only in `capi.rs`;
    // `Render::render_cached` documents that "the caller has already rejected" them. They are
    // repeated here so the tests call it under the same contract as `st_engine_render_bus`.
    fn render_bus_into(&mut self, bus: i32, sample_time: u64, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len() as u32;
        if bus < 0 || frames as usize > CACHE_CAPACITY || sample_time > u64::MAX - frames as u64 {
            left.fill(0.0);
            right.fill(0.0);
            return;
        }
        if frames != 0 {
            realtime(|| self.render.render_cached(self.shared, bus, false, sample_time, Some(left), Some(right), frames));
        }
    }
    /// `st_engine_clock_span`: `out` is written only when the result is `ST_CLOCK_OK`.
    fn clock_span(&mut self, graph_frame: i64, requested_frames: u32, upstream_latency_frames: i64, out: &mut STClockSpan) -> i32 {
        realtime(|| self.render.clock_span(self.shared, graph_frame, requested_frames, upstream_latency_frames, out))
    }
    /// `st_engine_capture_transport`
    fn capture_transport(&mut self) -> STTransportState {
        realtime(|| self.render.capture_transport(self.shared))
    }
    /// `st_engine_restore_transport`
    fn restore_transport(&mut self, state: STTransportState, resume: i32) {
        realtime(|| self.render.restore_transport(self.shared, &state, resume != 0));
    }
    /// `st_engine_forget_external_voices`
    fn forget_external_voices(&mut self, channel: i32) -> i32 {
        self.render.forget_external_voices(self.shared, channel)
    }
    /// `st_engine_pop_event` (the render thread's serialized consumer).
    fn pop_event(&mut self) -> Option<STEvent> {
        self.shared.events.pop()
    }
    /// `st_engine_reset_render_cache`, as the audio thread calls it between graph runs.
    fn reset_render_cache(&mut self) {
        self.shared.reset_render_cache();
    }
}

impl SharedHalf<'_> {
    /// `st_engine_status`
    pub fn status(&self) -> STStatus {
        self.shared.status()
    }
    /// `st_engine_track_peak`
    pub fn track_peak(&self, track: i32) -> f32 {
        self.shared.track_peak(track)
    }
    /// `st_engine_refresh_automation`
    pub fn refresh_automation(&self) {
        self.shared.refresh_automation();
    }
}

impl Engine {
    /// `st_engine_status`
    pub fn status(&self) -> STStatus {
        self.shared_half().status()
    }
    /// `st_engine_track_peak`
    pub fn track_peak(&self, track: i32) -> f32 {
        self.shared_half().track_peak(track)
    }
    /// `st_engine_refresh_automation`
    pub fn refresh_automation(&self) {
        self.shared_half().refresh_automation();
    }
}

// TODO(api): `st_status_transport_playing` and `st_status_count_in` are `static inline`
// functions of `TrackerEngine.h` with no Rust counterpart (they would be methods of
// `STStatus`); these are their bodies.

/// `st_status_transport_playing`: the transport as the control thread requested it.
pub fn status_transport_playing(status: &STStatus) -> i32 {
    if status.transport_pending != 0 {
        status.requested_playing
    } else {
        status.playing
    }
}

/// `st_status_count_in`
pub fn status_count_in(status: &STStatus) -> i32 {
    if status.transport_pending != 0 {
        0
    } else {
        status.count_in
    }
}

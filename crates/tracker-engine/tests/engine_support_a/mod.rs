//! Shared helpers for the engine integration tests `tests/engine_core_*.rs` and
//! `tests/engine_timing_*.rs`: ports of the former C++ programs `engine_tests.cpp` and
//! `engine_timing_tests.cpp`.
//!
//! * `Engine`: an owned engine (`tracker_engine::engine::Engine`) whose methods carry the names
//!   and argument order the C++ tests used, each a forward to the engine's Rust API.
//! * `realtime` and `assert_zero_render_allocations`: the C++ tests' replaced `operator new`
//!   with its thread-local "render in progress" flag.
//! * `tone`: the C++ tests' `engine_test_tone.hpp`.
//! * `core_helpers`: the fixtures and helpers of the core test programs.
#![allow(dead_code, clippy::too_many_arguments)]

pub mod core_helpers;
pub mod tone;

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use tracker_engine::engine::edits::Pcm;
use tracker_engine::engine::host;
use tracker_engine::engine::{STEvent, STInstrument, STStatus, STStep, STTrack};

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

/// Blocks freed by any thread while its render-in-progress flag was set. The C++ tests replaced
/// only `operator new` and so did not see these; the engine's contract ("never blocks, locks,
/// allocates or frees") covers them, and the count is asserted as well.
static REALTIME_FREES: AtomicU64 = AtomicU64::new(0);

mod allocator {
    use super::{REALTIME, REALTIME_ALLOCATIONS, REALTIME_FREES};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct CountingAllocator;

    #[inline]
    fn note(counter: &AtomicU64) {
        // `try_with`: the allocator also runs while a thread's locals are being torn down.
        if REALTIME.try_with(Cell::get).unwrap_or(false) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    // SAFETY: defers to the system allocator; the bookkeeping never allocates.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note(&REALTIME_ALLOCATIONS);
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            note(&REALTIME_ALLOCATIONS);
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            note(&REALTIME_ALLOCATIONS);
            unsafe { System.realloc(pointer, layout, size) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            note(&REALTIME_FREES);
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

/// Frees counted inside [`realtime`] sections so far, on every thread of this test binary.
pub fn realtime_frees() -> u64 {
    REALTIME_FREES.load(Ordering::Relaxed)
}

/// C++ `assert(realtimeAllocations.load()==0)` at the end of `main`. Every `#[test]` ends with
/// it, so each suite fails on its own when a render-thread call allocated (the counters are
/// process-wide, like the C++ one: once a render allocated, every later check fails too).
#[track_caller]
pub fn assert_zero_render_allocations() {
    assert_eq!(realtime_allocations(), 0, "a render-thread call allocated");
    assert_eq!(realtime_frees(), 0, "a render-thread call freed memory");
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
    engine: host::Engine,
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

/// The control-thread half of an engine: project edits, commit, transport and note commands.
pub struct ControlHalf<'a> {
    half: &'a mut host::ControlHalf,
}

/// The render-thread half of an engine: render, bus pulls, event consumption and the exclusive
/// stopped-graph operations.
pub struct RenderHalf<'a> {
    half: &'a mut host::RenderHalf,
    block: &'a mut Option<(u64, usize)>,
}

/// The part of an engine any thread may use: status, peaks and the refresh/reset mailboxes.
#[derive(Clone, Copy)]
pub struct SharedHalf<'a> {
    status: &'a host::StatusHandle,
}

impl Engine {
    /// C++ `Engine e;`: an engine at 48 kHz, asserted to exist.
    pub fn new() -> Engine {
        Engine::at(48000.0)
    }

    /// An engine at `rate`, asserted to exist.
    pub fn at(rate: f64) -> Engine {
        let engine = Engine::create(rate);
        assert!(engine.is_some());
        engine.unwrap()
    }

    /// An engine at `rate`: `None` where the engine refuses the rate.
    pub fn create(rate: f64) -> Option<Engine> {
        host::Engine::new(rate).map(|engine| Engine { engine, block: None })
    }

    /// The three thread-owned parts, for tests that drive the engine from several threads.
    pub fn split(&mut self) -> (ControlHalf<'_>, RenderHalf<'_>, SharedHalf<'_>) {
        let (control, render, status) = self.engine.halves();
        (ControlHalf { half: control }, RenderHalf { half: render, block: &mut self.block }, SharedHalf { status })
    }

    fn control_half(&mut self) -> ControlHalf<'_> {
        self.split().0
    }

    fn render_half(&mut self) -> RenderHalf<'_> {
        self.split().1
    }

    fn shared_half(&self) -> SharedHalf<'_> {
        SharedHalf { status: self.engine.status() }
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

    fn begin_update(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.begin_update(shared);
    }
    fn clear_project(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.clear_project();
    }
    /// `commit_update`: a rejected draft stays unpublished, silently.
    fn commit_update(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        let _ = control.commit(shared);
    }
    /// `commit_update_checked`: the error is the text the C function wrote into its
    /// error buffer.
    fn commit_update_checked(&mut self) -> Result<(), String> {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.commit(shared)
    }
    fn cancel_update(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.cancel_update();
    }
    fn collect(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.collect(shared);
    }

    // ---- project settings ----

    fn set_timing(&mut self, bpm: f64, lines_per_beat: i32, swing: f32, master_volume: f32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_timing(bpm, lines_per_beat, swing, master_volume);
    }
    fn set_metronome(&mut self, enabled: i32, numerator: i32, denominator: i32, volume: f32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_metronome(enabled != 0, numerator, denominator, volume);
    }
    fn set_track_count(&mut self, count: i32) -> bool {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_track_count(count)
    }
    fn set_track(&mut self, index: i32, track: STTrack) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_track(index, track);
    }
    fn set_host_mixing(&mut self, enabled: i32, computational_mask: u32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_host_mixing(enabled != 0, computational_mask);
    }
    fn set_instrument(&mut self, index: i32, instrument: STInstrument) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_instrument(index, instrument);
    }
    fn set_sequence(&mut self, patterns: &[i32], loop_song: i32, active_pattern: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_sequence(patterns, loop_song != 0, active_pattern);
    }
    fn set_effects(&mut self, delay_seconds: f32, delay_feedback: f32, delay_mix: f32, reverb_size: f32, reverb_damping: f32, reverb_mix: f32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_effects(delay_seconds, delay_feedback, delay_mix, reverb_size, reverb_damping, reverb_mix);
    }
    fn set_host_automation_map(&mut self, map: u64) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_host_automation_map(map);
    }

    // ---- patterns, steps and automation ----

    fn set_pattern(&mut self, index: i32, length: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_pattern(index, length);
    }
    fn reuse_pattern(&mut self, index: i32) -> bool {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.reuse_pattern(index)
    }
    /// `set_step`; an empty `chord` is the C call's `nullptr, 0`.
    fn set_step(&mut self, pattern: i32, track: i32, row: i32, step: STStep, chord: &[i32]) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_step(pattern, track, row, step, chord_list(chord));
    }
    fn set_step_column(&mut self, pattern: i32, track: i32, row: i32, column: i32, step: STStep) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_step_column(pattern, track, row, column, step);
    }
    fn clear_step(&mut self, pattern: i32, track: i32, row: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.clear_step(pattern, track, row);
    }
    fn clear_automation(&mut self, pattern: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.clear_automation(pattern);
    }
    fn add_automation_point(&mut self, pattern: i32, track: i32, target: i32, slot: i32, address: u64, interpolation: i32, row: f64, value: f64) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.add_automation_point(pattern, track, target, slot, address, interpolation, row, value);
    }
    fn add_host_automation_point(&mut self, pattern: i32, destination: i32, address: u64, interpolation: i32, row: f64, value: f64) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.add_host_automation_point(pattern, destination, address, interpolation, row, value);
    }

    // ---- samples, zones and timeline audio ----

    /// `set_sample(e, instrument, left, nullptr, frames, rate)`: mono PCM.
    fn set_sample(&mut self, instrument: i32, left: &[f32], rate: f64) -> bool {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_sample(shared, instrument, Some(Pcm { left, right: None, rate }))
    }
    /// `set_sample(e, instrument, nullptr, nullptr, 0, rate)`: remove the sample.
    fn clear_sample(&mut self, instrument: i32) -> bool {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_sample(shared, instrument, None)
    }
    fn set_slice_points(&mut self, instrument: i32, positions: &[f32]) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_slice_points(instrument, positions);
    }
    fn clear_zones(&mut self, instrument: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.clear_zones(instrument);
    }
    /// `add_sample_zone` with mono PCM.
    fn add_sample_zone(&mut self, instrument: i32, left: &[f32], rate: f64, key_low: i32, key_high: i32, velocity_low: i32, velocity_high: i32, root_note: i32, tune: f32, volume: f32) -> bool {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.add_sample_zone(
            shared,
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
    /// `set_audio_asset` with mono PCM.
    fn set_audio_asset(&mut self, asset: i32, left: &[f32], rate: f64) -> bool {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_audio_asset(shared, asset, Some(Pcm { left, right: None, rate }))
    }
    fn clear_audio_clips(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.clear_audio_clips();
    }
    fn add_audio_clip(&mut self, asset: i32, track: i32, start_beat: f64, duration_beats: f64, source_offset: f64, gain: f32, fade_in: f32, fade_out: f32, muted: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.add_audio_clip(asset, track, start_beat, duration_beats, source_offset, gain, fade_in, fade_out, muted != 0);
    }
    fn duration(&mut self, song_mode: i32) -> f64 {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.duration(song_mode != 0)
    }

    // ---- commands ----

    fn play(&mut self, song_mode: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.play(shared, song_mode);
    }
    fn stop(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.stop(shared);
    }
    fn play_count_in(&mut self, song_mode: i32, bars: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.play_count_in(shared, song_mode != 0, bars);
    }
    fn set_recording(&mut self, enabled: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_recording(shared, enabled != 0);
    }
    fn seek(&mut self, pattern: i32, row: i32, sequence: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.seek(shared, pattern, row, sequence);
    }
    fn set_pattern_loop(&mut self, start_row: i32, end_row: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.set_pattern_loop(shared, start_row, end_row);
    }
    fn queue_jump(&mut self, pattern: i32, sequence: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.queue_jump(shared, pattern, sequence);
    }
    fn queue_mode(&mut self, song_mode: i32, pattern: i32, sequence: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.queue_mode(shared, song_mode != 0, pattern, sequence);
    }
    fn note_on(&mut self, track: i32, instrument: i32, note: i32, velocity: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.note_on(shared, track, instrument, note, velocity);
    }
    fn note_off(&mut self, track: i32, note: i32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.note_off(shared, track, note);
    }
    /// `preview_step`; an empty `chord` is the C call's null pointer or zero count.
    fn preview_step(&mut self, track: i32, step: STStep, chord: &[i32], duration_seconds: f32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.preview_step(shared, track, step, chord_list(chord), duration_seconds);
    }
    fn panic(&mut self) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.panic(shared);
    }
    fn pitch_bend(&mut self, track: i32, semitones: f32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.pitch_bend(shared, track, semitones);
    }
    fn mod_wheel(&mut self, track: i32, value: f32) {
        #[allow(unused_variables)]
        let (control, shared, sample_rate) = self.half.parts();
        control.mod_wheel(shared, track, value);
    }
}

half_methods! { RenderHalf, render_half;
    /// A direct render into two buffers of equal length, counted as a render-thread call.
    fn render_into(&mut self, left: &mut [f32], right: &mut [f32]) {
        *self.block = None;
        realtime(|| self.half.render(left, right));
    }
    /// A render of no frames: apply queued commands without audio.
    fn render_nothing(&mut self) {
        *self.block = None;
        realtime(|| self.half.advance(0));
    }
    /// One bus of the block at a graph time (see `pull_block`), counted as a render-thread call.
    fn render_bus_into(&mut self, bus: i32, sample_time: u64, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len());
        realtime(|| {
            pull_block(self.half, self.block, sample_time, left.len());
            copy_bus(self.half, bus, left, right);
        });
    }
    /// The next event (the render thread is the serialized consumer).
    fn pop_event(&mut self) -> Option<STEvent> {
        self.half.pop_event()
    }
}

impl SharedHalf<'_> {
    pub fn status(&self) -> STStatus {
        self.status.status()
    }
    pub fn track_peak(&self, track: i32) -> f32 {
        self.status.track_peak(track)
    }
    pub fn refresh_automation(&self) {
        self.status.refresh_automation();
    }
}

impl Engine {
    pub fn status(&self) -> STStatus {
        self.shared_half().status()
    }
    pub fn track_peak(&self, track: i32) -> f32 {
        self.shared_half().track_peak(track)
    }
    pub fn refresh_automation(&self) {
        self.shared_half().refresh_automation();
    }
}

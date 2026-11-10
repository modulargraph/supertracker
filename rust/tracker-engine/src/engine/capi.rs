//! The C API of `Sources/TrackerEngine/include/TrackerEngine.h`: the `extern "C"` block of
//! `TrackerEngine.cpp`.
//!
//! This file only converts C arguments (null checks, pointers to slices, error buffers) and
//! forwards to the safe Rust methods of the engine's three parts: [`Control`] (`edits.rs`,
//! `control.rs`), [`Render`] and [`Shared`]. No engine behaviour lives here, and no function
//! unwinds.
//!
//! Threading contract (from the header): ordinary control functions are single-producer and run
//! on one serial control thread; render/pop_event are serialized audio-thread operations; status
//! and peak getters are atomic snapshots readable from any thread; capture/restore transport and
//! forget_external_voices are exclusive stopped-graph operations.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::too_many_arguments)]

use super::abi::*;
use super::control::Control;
use super::edits::Pcm;
use super::state::*;
use super::tables::CACHE_CAPACITY;
use super::util::cmin;
use std::ffi::c_char;

/// The engine behind a C handle, or `None` for a null handle.
///
/// # Safety
/// A non-null `engine` must come from `st_engine_create` and not have been destroyed.
#[inline(always)]
unsafe fn engine<'a>(engine: *mut STEngine) -> Option<&'a STEngine> {
    // SAFETY: per this function's contract the pointer is null or a live engine. Only a shared
    // reference is formed: the two thread-owned halves sit in `UnsafeCell`s.
    unsafe { engine.as_ref() }
}

/// The control-thread half of the engine.
#[allow(clippy::mut_from_ref)]
#[inline(always)]
fn control(e: &STEngine) -> &mut Control {
    // SAFETY: only the single serial control thread calls the functions that use this accessor
    // (API contract: "ordinary control functions are single-producer; use one serial control
    // queue"), each takes it once and does not keep it past its return, so no two references to
    // the control state coexist.
    unsafe { &mut *e.control.get() }
}

/// The render-thread half of the engine.
#[allow(clippy::mut_from_ref)]
#[inline(always)]
fn render(e: &STEngine) -> &mut Render {
    // SAFETY: only the serialized render functions and the exclusive stopped-graph operations
    // call the functions that use this accessor (API contract), each takes it once and does not
    // keep it past its return, so no two references to the render state coexist.
    unsafe { &mut *e.render.get() }
}

/// Copy `message` into the caller's error buffer (truncated, NUL-terminated).
///
/// # Safety
/// `error` must be null or writable for `capacity` bytes.
unsafe fn write_error(error: *mut c_char, capacity: u32, message: &str) {
    if error.is_null() || capacity == 0 {
        return;
    }
    let count = cmin(capacity as usize - 1, message.len());
    // SAFETY: `count + 1 <= capacity` bytes are written into the caller's buffer.
    unsafe {
        std::ptr::copy_nonoverlapping(message.as_ptr(), error as *mut u8, count);
        *error.add(count) = 0;
    }
}

/// Clear the error buffer, run `operation`, report its error text. Returns 1 on success.
///
/// # Safety
/// As `write_error`.
unsafe fn checked(error: *mut c_char, capacity: u32, operation: impl FnOnce() -> Result<(), String>) -> i32 {
    // SAFETY: per this function's contract.
    unsafe {
        write_error(error, capacity, "");
        match operation() {
            Ok(()) => 1,
            Err(problem) => {
                write_error(error, capacity, &problem);
                0
            }
        }
    }
}

/// `count` values at `list`, or an empty list for a null pointer or a non-positive count.
///
/// # Safety
/// A non-null `list` must point to `count` values.
unsafe fn list<'a, T>(list: *const T, count: i64) -> &'a [T] {
    if list.is_null() || count <= 0 {
        &[]
    } else {
        // SAFETY: per this function's contract.
        unsafe { std::slice::from_raw_parts(list, count as usize) }
    }
}

/// The caller's clock event array, or an empty list when it is missing or its count is not one
/// the clock accepts (1..=1024); the clock rejects an empty list with the same message, without
/// the caller's memory being read.
///
/// # Safety
/// A non-null `events` must point to `count` events.
unsafe fn clock_events<'a, T>(events: *const T, count: u32) -> &'a [T] {
    // SAFETY: per this function's contract.
    unsafe { list(events, if count > 1024 { 0 } else { count as i64 }) }
}

/// A chord as the setters take it: `None` unless a list of at least one note was passed.
///
/// # Safety
/// A non-null `chord` must point to `count` notes.
unsafe fn chord_list<'a>(chord: *const i32, count: i32) -> Option<&'a [i32]> {
    // SAFETY: per this function's contract; at most ST_MAX_CHORD notes are read.
    let notes = unsafe { list(chord, cmin(count, ST_MAX_CHORD as i32) as i64) };
    (!notes.is_empty()).then_some(notes)
}

/// Planar PCM arguments: `Ok(None)` for the "clear" form (null left, zero frames), `Err(())` for
/// input the engine rejects before reading it.
///
/// # Safety
/// A non-null `left`/`right` must point to `frames` floats.
unsafe fn pcm<'a>(left: *const f32, right: *const f32, frames: u64, rate: f64) -> Result<Option<Pcm<'a>>, ()> {
    if left.is_null() && frames == 0 {
        return Ok(None);
    }
    if left.is_null() || frames < 1 || frames > i32::MAX as u64 {
        return Err(());
    }
    // SAFETY: per this function's contract.
    unsafe {
        let left = std::slice::from_raw_parts(left, frames as usize);
        let right = if right.is_null() { None } else { Some(std::slice::from_raw_parts(right, frames as usize)) };
        Ok(Some(Pcm { left, right, rate }))
    }
}

/// An output buffer of `frames` floats, or `None` for a null pointer.
///
/// # Safety
/// A non-null `pointer` must be writable for `frames` floats and not overlap any other buffer
/// turned into a slice for the same call.
#[inline(always)]
unsafe fn output<'a>(pointer: *mut f32, frames: u32) -> Option<&'a mut [f32]> {
    if pointer.is_null() {
        None
    } else {
        // SAFETY: per this function's contract.
        Some(unsafe { std::slice::from_raw_parts_mut(pointer, frames as usize) })
    }
}

/// The caller's stereo output buffers. When both pointers name the same buffer, the C++ engine's
/// result is the right channel (written last), so only that one is kept.
///
/// # Safety
/// Non-null pointers must be writable for `frames` floats; distinct buffers must not overlap.
#[inline(always)]
unsafe fn outputs<'a>(l: *mut f32, r: *mut f32, frames: u32) -> (Option<&'a mut [f32]>, Option<&'a mut [f32]>) {
    // SAFETY: per this function's contract; the same-buffer case forms only one slice.
    unsafe {
        if l == r {
            (None, output(r, frames))
        } else {
            (output(l, frames), output(r, frames))
        }
    }
}

fn silence(l: Option<&mut [f32]>, r: Option<&mut [f32]>) {
    for out in [l, r].into_iter().flatten() {
        out.fill(0.0);
    }
}

#[no_mangle]
pub extern "C" fn st_default_track() -> STTrack {
    default_track()
}

#[no_mangle]
pub extern "C" fn st_default_instrument() -> STInstrument {
    default_instrument()
}

#[no_mangle]
pub extern "C" fn st_default_step() -> STStep {
    default_step()
}

#[no_mangle]
pub extern "C" fn st_engine_create(sample_rate: f64) -> *mut STEngine {
    match STEngine::create(sample_rate) {
        Some(engine) => Box::into_raw(engine),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_destroy(e: *mut STEngine) {
    if !e.is_null() {
        // SAFETY: the handle came from `st_engine_create` (`Box::into_raw`) and, per the API
        // contract, is destroyed once, after the audio graph has stopped calling the engine.
        drop(unsafe { Box::from_raw(e) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_begin_update(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).begin_update(&e.shared);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_project(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).clear_project();
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_timing(e: *mut STEngine, bpm: f64, lines_per_beat: i32, swing: f32, master_volume: f32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_timing(bpm, lines_per_beat, swing, master_volume);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_frozen_song_policy(
    e: *mut STEngine,
    enabled: i32,
    publication_id: u64,
    error: *mut c_char,
    error_capacity: u32,
) -> i32 {
    let e = unsafe { engine(e) };
    unsafe {
        checked(error, error_capacity, || match e {
            Some(e) => control(e).set_frozen_song_policy(&e.shared, enabled, publication_id),
            None => Err("Frozen song policy requires a valid engine, boolean enable flag and nonzero publication ID.".into()),
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_frozen_song_extent(
    e: *mut STEngine,
    publication_id: u64,
    musical_frames: i64,
    error: *mut c_char,
    error_capacity: u32,
) -> i32 {
    let e = unsafe { engine(e) };
    unsafe {
        checked(error, error_capacity, || match e {
            Some(e) => control(e).set_frozen_song_extent(publication_id, musical_frames),
            None => Err("Frozen song extent requires a valid engine, nonzero publication and1..100000000 frames.".into()),
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_timing_map(
    e: *mut STEngine,
    tempos: *const STClockTempoEvent,
    tempo_count: u32,
    meters: *const STClockMeterEvent,
    meter_count: u32,
    revision: u64,
    error: *mut c_char,
    error_capacity: u32,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else {
        unsafe { write_error(error, error_capacity, "") };
        return 0;
    };
    // SAFETY: the caller passes complete arrays of the stated counts (API contract). When either
    // list is missing both are passed as empty, as the C++ clock rejected that case as a whole.
    let (tempo_list, meter_list) = unsafe {
        if tempos.is_null() || meters.is_null() {
            (&[][..], &[][..])
        } else {
            (clock_events(tempos, tempo_count), clock_events(meters, meter_count))
        }
    };
    unsafe {
        checked(error, error_capacity, || control(e).set_timing_map(e.sample_rate, tempo_list, meter_list, revision))
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_timing_map(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).clear_timing_map();
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_pattern_timing_origin(e: *mut STEngine, beat: f64) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_pattern_timing_origin(beat) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_metronome(e: *mut STEngine, enabled: i32, numerator: i32, denominator: i32, volume: f32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_metronome(enabled != 0, numerator, denominator, volume);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_track_count(e: *mut STEngine, count: i32) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_track_count(count) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_track(e: *mut STEngine, index: i32, track: STTrack) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_track(index, track);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_track_voice(
    e: *mut STEngine,
    track: i32,
    choke_group: i32,
    choke_scope: i32,
    voice_mode: i32,
) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_track_voice(track, choke_group, choke_scope, voice_mode) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_host_mixing(e: *mut STEngine, enabled: i32, computational_mask: u32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_host_mixing(enabled != 0, computational_mask);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_host_mixing_tracks(
    e: *mut STEngine,
    enabled: i32,
    words: *const u64,
    word_count: u32,
    enabled_beyond: i32,
) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    if word_count != 0 && words.is_null() {
        return;
    }
    // SAFETY: `words` holds `word_count` words (API contract).
    let words = unsafe { list(words, word_count as i64) };
    control(e).set_host_mixing_tracks(enabled != 0, words, enabled_beyond != 0);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_instrument_count(e: *mut STEngine, count: i32) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_instrument_count(count) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_instrument(e: *mut STEngine, index: i32, instrument: STInstrument) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_instrument(index, instrument);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_pattern(e: *mut STEngine, index: i32, length: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_pattern(index, length);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_reuse_pattern(e: *mut STEngine, index: i32) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).reuse_pattern(index) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_pattern_epoch(e: *mut STEngine) -> u64 {
    match unsafe { engine(e) } {
        Some(e) => control(e).pattern_epoch(),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_step(
    e: *mut STEngine,
    pattern: i32,
    track: i32,
    row: i32,
    step: STStep,
    chord: *const i32,
    chord_count: i32,
) {
    if let Some(e) = unsafe { engine(e) } {
        // SAFETY: a non-null `chord` holds `chord_count` notes (API contract).
        let chord = unsafe { chord_list(chord, chord_count) };
        control(e).set_step(pattern, track, row, step, chord);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_plugin_step_fx(
    e: *mut STEngine,
    pattern: i32,
    track: i32,
    row: i32,
    column: i32,
    slot: i32,
    host_destination: i32,
    address: u64,
    value: f64,
) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_plugin_step_fx(pattern, track, row, column, slot, host_destination, address, value) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_step(e: *mut STEngine, pattern: i32, track: i32, row: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).clear_step(pattern, track, row);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_step_column(e: *mut STEngine, pattern: i32, track: i32, row: i32, column: i32, step: STStep) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_step_column(pattern, track, row, column, step);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_automation(e: *mut STEngine, pattern: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).clear_automation(pattern);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_add_automation_point(
    e: *mut STEngine,
    pattern: i32,
    track: i32,
    target: i32,
    slot: i32,
    address: u64,
    interpolation: i32,
    row: f64,
    value: f64,
) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).add_automation_point(pattern, track, target, slot, address, interpolation, row, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_add_host_automation_point(
    e: *mut STEngine,
    pattern: i32,
    destination: i32,
    address: u64,
    interpolation: i32,
    row: f64,
    value: f64,
) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).add_host_automation_point(pattern, destination, address, interpolation, row, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_host_automation_map(e: *mut STEngine, map: u64) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_host_automation_map(map);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_sequence(
    e: *mut STEngine,
    patterns: *const i32,
    count: i32,
    loop_song: i32,
    active_pattern: i32,
) {
    if let Some(e) = unsafe { engine(e) } {
        // SAFETY: a non-null `patterns` holds `count` entries (API contract); at most 4096 are used.
        let patterns = unsafe { list(patterns, cmin(count, 4096) as i64) };
        control(e).set_sequence(patterns, loop_song != 0, active_pattern);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_effects(
    e: *mut STEngine,
    delay_seconds: f32,
    delay_feedback: f32,
    delay_mix: f32,
    reverb_size: f32,
    reverb_damping: f32,
    reverb_mix: f32,
) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_effects(delay_seconds, delay_feedback, delay_mix, reverb_size, reverb_damping, reverb_mix);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_sample(
    e: *mut STEngine,
    instrument: i32,
    left: *const f32,
    right: *const f32,
    frames: u64,
    sample_rate: f64,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    // SAFETY: `left` (and a non-null `right`) hold `frames` floats (API contract).
    match unsafe { pcm(left, right, frames, sample_rate) } {
        Ok(pcm) => control(e).set_sample(&e.shared, e.sample_rate, instrument, pcm) as i32,
        Err(()) => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_slice_points(e: *mut STEngine, instrument: i32, positions: *const f32, count: i32) {
    if let Some(e) = unsafe { engine(e) } {
        // SAFETY: a non-null `positions` holds `count` values (API contract); at most 128 are used.
        let positions = unsafe { list(positions, cmin(count, 128) as i64) };
        control(e).set_slice_points(instrument, positions);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_sample_cycle(e: *mut STEngine, instrument: i32, frames: i32) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_sample_cycle(&e.shared, e.sample_rate, instrument, frames) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_zones(e: *mut STEngine, instrument: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).clear_zones(instrument);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_add_sample_zone(
    e: *mut STEngine,
    instrument: i32,
    left: *const f32,
    right: *const f32,
    frames: u64,
    sample_rate: f64,
    key_low: i32,
    key_high: i32,
    velocity_low: i32,
    velocity_high: i32,
    root_note: i32,
    tune: f32,
    volume: f32,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    // SAFETY: `left` (and a non-null `right`) hold `frames` floats (API contract).
    let Ok(Some(pcm)) = (unsafe { pcm(left, right, frames, sample_rate) }) else { return 0 };
    control(e).add_sample_zone(
        &e.shared,
        e.sample_rate,
        instrument,
        pcm,
        key_low,
        key_high,
        velocity_low,
        velocity_high,
        root_note,
        tune,
        volume,
    ) as i32
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_audio_asset(
    e: *mut STEngine,
    asset: i32,
    left: *const f32,
    right: *const f32,
    frames: u64,
    sample_rate: f64,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let ctl = control(e);
    // SAFETY: `left` (and a non-null `right`) hold `frames` floats (API contract).
    match unsafe { pcm(left, right, frames, sample_rate) } {
        Ok(pcm) => ctl.set_audio_asset(&e.shared, e.sample_rate, asset, pcm) as i32,
        Err(()) => {
            // The C++ engine opened the draft before rejecting the PCM.
            if (0..256).contains(&asset) {
                ctl.edit();
            }
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_sample_zone_loop_enabled(e: *mut STEngine, instrument: i32, zone_index: i32, enabled: i32) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_sample_zone_loop_enabled(instrument, zone_index, enabled) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_sample_zone_region(
    e: *mut STEngine,
    instrument: i32,
    zone_index: i32,
    start: f32,
    end: f32,
    loop_start: f32,
    loop_end: f32,
    pan: f32,
) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_sample_zone_region(instrument, zone_index, start, end, loop_start, loop_end, pan) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_audio_clips(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).clear_audio_clips();
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_add_song_tail_clip(e: *mut STEngine, asset: i32, track: i32, start_frame: i64, gain: f32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).add_song_tail_clip(asset, track, start_frame, gain);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_add_pattern_clip(
    e: *mut STEngine,
    head_asset: i32,
    loop_asset: i32,
    track: i32,
    pattern: i32,
    gain: f32,
) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).add_pattern_clip(head_asset, loop_asset, track, pattern, gain);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_add_audio_clip(
    e: *mut STEngine,
    asset: i32,
    track: i32,
    start_beat: f64,
    duration_beats: f64,
    source_offset: f64,
    gain: f32,
    fade_in: f32,
    fade_out: f32,
    muted: i32,
) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).add_audio_clip(asset, track, start_beat, duration_beats, source_offset, gain, fade_in, fade_out, muted != 0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_commit_update(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        let _ = control(e).commit(&e.shared, e.sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_commit_update_checked(e: *mut STEngine, error: *mut c_char, error_capacity: u32) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    unsafe { checked(error, error_capacity, || control(e).commit(&e.shared, e.sample_rate)) }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_cancel_update(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).cancel_update();
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_collect(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).collect(&e.shared);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_play(e: *mut STEngine, song_mode: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).play(&e.shared, song_mode);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_song_loop(e: *mut STEngine, start_index: i32, end_index: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_song_loop(start_index, end_index);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_send_effects(e: *mut STEngine, delay_beats: f64, delay_tone: f32, reverb_predelay: f32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_send_effects(e.sample_rate, delay_beats, delay_tone, reverb_predelay);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_pattern_loop(e: *mut STEngine, start_row: i32, end_row: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_pattern_loop(&e.shared, start_row, end_row);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_stop(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).stop(&e.shared);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_recording(e: *mut STEngine, enabled: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).set_recording(&e.shared, enabled != 0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_seek(e: *mut STEngine, pattern: i32, row: i32, sequence: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).seek(&e.shared, pattern, row, sequence);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_queue_jump(e: *mut STEngine, pattern: i32, sequence: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).queue_jump(&e.shared, pattern, sequence);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_queue_mode(e: *mut STEngine, song_mode: i32, pattern: i32, sequence: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).queue_mode(&e.shared, song_mode != 0, pattern, sequence);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_play_count_in(e: *mut STEngine, song_mode: i32, bars: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).play_count_in(&e.shared, song_mode != 0, bars);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_capture_transport(e: *mut STEngine) -> STTransportState {
    match unsafe { engine(e) } {
        Some(e) => render(e).capture_transport(&e.shared),
        None => STTransportState::default(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_restore_transport(e: *mut STEngine, state: STTransportState, resume: i32) {
    if let Some(e) = unsafe { engine(e) } {
        render(e).restore_transport(&e.shared, &state, resume != 0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_note_on(e: *mut STEngine, track: i32, instrument: i32, note: i32, velocity: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).note_on(&e.shared, track, instrument, note, velocity);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_note_off(e: *mut STEngine, track: i32, note: i32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).note_off(&e.shared, track, note);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_input_note(
    e: *mut STEngine,
    request_id: u64,
    edge: i32,
    track: i32,
    instrument: i32,
    note: i32,
    velocity: i32,
    apply_note: i32,
) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).input_note(&e.shared, request_id, edge, track, instrument, note, velocity, apply_note) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_pop_input_ack(e: *mut STEngine, ack: *mut STInputAck) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if ack.is_null() {
        return 0;
    }
    match e.shared.pop_input_ack() {
        Some(value) => {
            // SAFETY: `ack` is a non-null pointer to caller storage (API contract).
            unsafe { ack.write(value) };
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_preview_step(
    e: *mut STEngine,
    track: i32,
    step: STStep,
    chord: *const i32,
    chord_count: i32,
    duration_seconds: f32,
) {
    if let Some(e) = unsafe { engine(e) } {
        // SAFETY: a non-null `chord` holds `chord_count` notes (API contract).
        let chord = unsafe { chord_list(chord, chord_count) };
        control(e).preview_step(&e.shared, track, step, chord, duration_seconds);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_panic(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).panic(&e.shared);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_forget_external_voices(e: *mut STEngine, channel: i32) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => render(e).forget_external_voices(&e.shared, channel),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_pitch_bend(e: *mut STEngine, track: i32, semitones: f32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).pitch_bend(&e.shared, track, semitones);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_mod_wheel(e: *mut STEngine, track: i32, value: f32) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).mod_wheel(&e.shared, track, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_instrument_parameter(e: *mut STEngine, instrument: i32, target: i32, value: f32) -> i32 {
    match unsafe { engine(e) } {
        Some(e) => control(e).set_instrument_parameter(&e.shared, instrument, target, value) as i32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_refresh_automation(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.refresh_automation();
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_render(e: *mut STEngine, left: *mut f32, right: *mut f32, frames: u32) {
    // SAFETY: the caller's buffers hold `frames` floats each (API contract).
    let (l, r) = unsafe { outputs(left, right, frames) };
    match unsafe { engine(e) } {
        Some(e) => render(e).render(&e.shared, l, r, frames),
        None => silence(l, r),
    }
}

/// `renderCached`'s argument checks; the cached render itself is `Render::render_cached`.
///
/// # Safety
/// As the public render functions: a live or null engine, buffers of `frames` floats.
unsafe fn render_cached(e: *mut STEngine, bus: i32, channel: bool, time: u64, left: *mut f32, right: *mut f32, frames: u32) {
    // SAFETY: the caller's buffers hold `frames` floats each (API contract).
    let (l, r) = unsafe { outputs(left, right, frames) };
    let e = unsafe { engine(e) };
    let Some(e) = e.filter(|_| !(!channel && bus < 0) && frames as usize <= CACHE_CAPACITY && time <= u64::MAX - frames as u64)
    else {
        silence(l, r);
        return;
    };
    if frames != 0 {
        render(e).render_cached(&e.shared, bus, channel, time, l, r, frames);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_render_bus(e: *mut STEngine, bus: i32, sample_time: u64, left: *mut f32, right: *mut f32, frames: u32) {
    unsafe { render_cached(e, bus, false, sample_time, left, right, frames) }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_render_channel(
    e: *mut STEngine,
    channel: i32,
    sample_time: u64,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    unsafe { render_cached(e, channel, true, sample_time, left, right, frames) }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_reset_render_cache(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.reset_render_cache();
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clock_span(
    e: *mut STEngine,
    graph_frame: i64,
    requested_frames: u32,
    upstream_latency_frames: i64,
    span: *mut STClockSpan,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return ST_CLOCK_INVALID };
    if span.is_null() {
        return ST_CLOCK_INVALID;
    }
    let mut result = STClockSpan::default();
    let status = render(e).clock_span(&e.shared, graph_frame, requested_frames, upstream_latency_frames, &mut result);
    if status == ST_CLOCK_OK {
        // SAFETY: `span` is a non-null pointer to caller storage (API contract); it is written
        // only on success ("output is untouched on error").
        unsafe { span.write(result) };
    }
    status
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_freeze_span(
    e: *mut STEngine,
    graph_frame: i64,
    requested_frames: u32,
    upstream_latency_frames: i64,
    span: *mut STFreezeSpan,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return ST_CLOCK_INVALID };
    if span.is_null() {
        return ST_CLOCK_INVALID;
    }
    let mut result = STFreezeSpan::default();
    let status = render(e).freeze_span(&e.shared, graph_frame, requested_frames, upstream_latency_frames, &mut result);
    if status == ST_CLOCK_OK {
        // SAFETY: as in `st_engine_clock_span`.
        unsafe { span.write(result) };
    }
    status
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_pop_event(e: *mut STEngine, event: *mut STEvent) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if event.is_null() {
        return 0;
    }
    match e.shared.events.pop() {
        Some(value) => {
            // SAFETY: `event` is a non-null pointer to caller storage (API contract).
            unsafe { event.write(value) };
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_status(e: *mut STEngine) -> STStatus {
    match unsafe { engine(e) } {
        Some(e) => e.shared.status(),
        None => STStatus::default(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_watch_voice_positions(e: *mut STEngine, instrument: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.watch_voice_positions(instrument);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_voice_positions(
    e: *mut STEngine,
    voices: *mut STVoicePosition,
    capacity: i32,
    serial: *mut u64,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if voices.is_null() || capacity <= 0 {
        return 0;
    }
    // SAFETY: `voices` is caller storage for `capacity` entries (API contract); no more than the
    // snapshot capacity is ever written.
    let out = unsafe { std::slice::from_raw_parts_mut(voices, cmin(capacity as usize, ST_VOICE_POSITION_CAPACITY)) };
    match e.shared.voice_positions(out) {
        Some((count, published)) => {
            if !serial.is_null() {
                // SAFETY: `serial` is a non-null pointer to caller storage (API contract).
                unsafe { serial.write(published) };
            }
            count as i32
        }
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_track_peak(e: *mut STEngine, track: i32) -> f32 {
    match unsafe { engine(e) } {
        Some(e) => e.shared.track_peak(track),
        None => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_duration(e: *mut STEngine, song_mode: i32) -> f64 {
    match unsafe { engine(e) } {
        Some(e) => control(e).duration(e.sample_rate, song_mode != 0),
        None => 0.0,
    }
}

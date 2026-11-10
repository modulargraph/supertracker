//! The C API of `Sources/TrackerEngine/include/TrackerEngine.h`: the `extern "C"` block of
//! `TrackerEngine.cpp`. Every function validates its input as the C++ engine did and never
//! unwinds.
//!
//! Threading contract (from the header): ordinary control functions are single-producer and run
//! on one serial control thread; render/pop_event are serialized audio-thread operations; status
//! and peak getters are atomic snapshots readable from any thread; capture/restore transport and
//! forget_external_voices are exclusive stopped-graph operations.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::too_many_arguments)]

use super::abi::*;
use super::timing::EngineTiming;
use super::control::{Control, BAD_ALLOC};
use super::project::*;
use super::state::*;
use super::tables::{FreezeHistory, CACHE_CAPACITY};
use super::util::{clamp, cmin, finite, finite64};
use super::voice::VoiceSink;
use std::ffi::c_char;
use std::sync::atomic::{fence, AtomicI32, Ordering};
use std::sync::Arc;

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

/// # Safety
/// As `write_error`.
unsafe fn clear_error(error: *mut c_char, capacity: u32) {
    if !error.is_null() && capacity != 0 {
        // SAFETY: the buffer holds at least one byte.
        unsafe { *error = 0 };
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

/// The caller's clock event array, or an empty list when it is missing or its count is not one
/// the clock accepts (1..=1024).
///
/// # Safety
/// A non-null `list` must point to `count` events.
unsafe fn events<'a, T>(list: *const T, count: u32) -> &'a [T] {
    if list.is_null() || count == 0 || count > 1024 {
        &[]
    } else {
        // SAFETY: per this function's contract.
        unsafe { std::slice::from_raw_parts(list, count as usize) }
    }
}

/// Copy planar PCM into a new sample (right defaults to left), sanitizing every value.
///
/// # Safety
/// `left` must point to `frames` floats; `right` must be null or point to `frames` floats.
unsafe fn make_sample(left: *const f32, right: *const f32, frames: u64, rate: f64) -> Option<Box<Sample>> {
    let count = frames as usize;
    let (mut l, mut r) = (Vec::new(), Vec::new());
    // Out of memory is a failed load (C++ `catch(...)`), not an abort.
    l.try_reserve_exact(count).ok()?;
    r.try_reserve_exact(count).ok()?;
    // SAFETY: per this function's contract.
    let (source_l, source_r) = unsafe {
        let source_l = std::slice::from_raw_parts(left, count);
        (source_l, if right.is_null() { source_l } else { std::slice::from_raw_parts(right, count) })
    };
    l.extend(source_l.iter().map(|&x| clamp(finite(x, 0.0), -8.0, 8.0)));
    r.extend(source_r.iter().map(|&x| clamp(finite(x, 0.0), -8.0, 8.0)));
    Some(Box::new(Sample { refs: AtomicI32::new(0), l, r, rate }))
}

fn valid_pcm(left: *const f32, frames: u64, rate: f64) -> bool {
    !left.is_null() && frames >= 1 && frames <= i32::MAX as u64 && rate.is_finite() && rate >= 1000.0 && rate <= 768000.0
}

/// Give the new sample to the engine's sample list and return a counted reference to it.
fn adopt_sample(ctl: &mut Control, sample: Box<Sample>) -> Option<SampleRef> {
    let raw: *const Sample = &*sample;
    ctl.samples.push(sample);
    // SAFETY: `raw` points into the box just pushed onto `samples`, which keeps it alive (and at
    // the same address) until `collect()` finds its count at zero.
    unsafe { SampleRef::retain(raw) }
}

/// "Publishes at once outside an update": commit unless a begin/commit transaction is open.
fn publish_outside_update(e: &STEngine, ctl: &mut Control) -> i32 {
    if !ctl.updating && ctl.commit(&e.shared, e.sample_rate).is_err() {
        return 0;
    }
    1
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    let ctl = control(e);
    ctl.collect(&e.shared);
    ctl.draft = Some(Box::new((*ctl.control).clone()));
    ctl.updating = true;
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_project(e: *mut STEngine) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let ctl = control(e);
    let mut fresh = Box::new(Project::new());
    fresh.pattern_epoch = 0;
    fresh.instruments = ctl.control.instruments.clone();
    fresh.audio_assets = ctl.control.audio_assets.clone();
    ctl.draft = Some(fresh);
    ctl.updating = true;
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_timing(e: *mut STEngine, bpm: f64, lines_per_beat: i32, swing: f32, master_volume: f32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    p.bpm = clamp(finite64(bpm, 120.0), 20.0, 999.0);
    p.lpb = clamp(lines_per_beat, 1, 32);
    p.swing = clamp(finite(swing, 0.0), 0.0, 0.95);
    p.master = clamp(finite(master_volume, 0.7), 0.0, 2.0);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_frozen_song_policy(
    e: *mut STEngine,
    enabled: i32,
    publication_id: u64,
    error: *mut c_char,
    error_capacity: u32,
) -> i32 {
    unsafe { clear_error(error, error_capacity) };
    let e = unsafe { engine(e) };
    let Some(e) = e.filter(|_| (enabled == 0 || enabled == 1) && !(enabled != 0 && publication_id == 0)) else {
        unsafe {
            write_error(
                error,
                error_capacity,
                "Frozen song policy requires a valid engine, boolean enable flag and nonzero publication ID.",
            )
        };
        return 0;
    };
    // Allocate on control before publishing the policy. Storage lives until quiescent engine
    // destruction, including rejected/discarded drafts.
    if enabled != 0 && e.shared.freeze_history.load(Ordering::Acquire).is_null() {
        let Some(history) = FreezeHistory::try_new() else {
            unsafe { write_error(error, error_capacity, BAD_ALLOC) };
            return 0;
        };
        e.shared.freeze_history.store(Box::into_raw(history), Ordering::Release);
    }
    let p = match control(e).try_edit() {
        Ok(p) => p,
        Err(problem) => {
            unsafe { write_error(error, error_capacity, &problem) };
            return 0;
        }
    };
    if enabled == 0 || !p.frozen_song_policy || p.frozen_publication_id != publication_id {
        p.frozen_song_frames = 0;
    }
    p.frozen_song_policy = enabled != 0;
    p.frozen_publication_id = if enabled != 0 { publication_id } else { 0 };
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_frozen_song_extent(
    e: *mut STEngine,
    publication_id: u64,
    musical_frames: i64,
    error: *mut c_char,
    error_capacity: u32,
) -> i32 {
    unsafe { clear_error(error, error_capacity) };
    let fail = |message: &str| {
        unsafe { write_error(error, error_capacity, message) };
        0
    };
    let e = unsafe { engine(e) };
    let Some(e) = e.filter(|_| publication_id != 0 && musical_frames >= 1 && musical_frames <= 100000000) else {
        return fail("Frozen song extent requires a valid engine, nonzero publication and1..100000000 frames.");
    };
    let ctl = control(e);
    let previous = ctl.view();
    if !previous.frozen_song_policy || previous.frozen_publication_id != publication_id {
        return fail("Frozen song extent must match the enabled publication ID.");
    }
    if previous.frozen_song_frames != 0 && previous.frozen_song_frames != musical_frames {
        return fail("A frozen publication cannot change its exact song extent; create a new publication.");
    }
    match ctl.try_edit() {
        Ok(p) => p.frozen_song_frames = musical_frames,
        Err(problem) => return fail(&problem),
    }
    1
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
    unsafe { clear_error(error, error_capacity) };
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let ctl = control(e);
    // SAFETY: the caller passes complete arrays of the stated counts (API contract). A missing
    // list or a count outside 1..=1024 is passed on as an empty list, which the clock rejects
    // with the same message without reading the caller's memory.
    let (tempo_list, meter_list) = unsafe { (events(tempos, tempo_count), events(meters, meter_count)) };
    let (tempo_list, meter_list) = if tempos.is_null() || meters.is_null() { (&[][..], &[][..]) } else { (tempo_list, meter_list) };
    let map = match EngineTiming::new(e.sample_rate, tempo_list, meter_list, revision) {
        Ok(map) => Arc::new(map),
        Err(problem) => {
            unsafe { write_error(error, error_capacity, &problem) };
            return 0;
        }
    };
    let map = match &ctl.view().timing {
        Some(existing) if existing.matches(&map) => existing.clone(),
        _ => {
            ctl.timing_maps.push(map.clone());
            map
        }
    };
    ctl.edit().timing = Some(map);
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_timing_map(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).edit().timing = None;
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_pattern_timing_origin(e: *mut STEngine, beat: f64) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if !beat.is_finite() || beat < 0.0 || beat > 1000000.0 {
        return 0;
    }
    control(e).edit().pattern_origin_beat = beat;
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_metronome(e: *mut STEngine, enabled: i32, numerator: i32, denominator: i32, volume: f32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    p.metronome = enabled != 0;
    p.numerator = clamp(numerator, 1, 16);
    p.denominator = if matches!(denominator, 2 | 4 | 8 | 16) { denominator } else { 4 };
    p.metronome_volume = clamp(finite(volume, 0.3), 0.0, 1.0);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_track_count(e: *mut STEngine, count: i32) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if count < 1 || count > ST_TRACK_SANITY_LIMIT {
        return 0;
    }
    let p = control(e).edit();
    p.track_count = count;
    let base = p.return_bus() as usize;
    if p.tracks.len() < base {
        p.tracks.resize(base, default_track());
    }
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_track(e: *mut STEngine, index: i32, mut track: STTrack) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    if index < 0 || index >= ST_TRACK_SANITY_LIMIT {
        return;
    }
    let ctl = control(e);
    track.volume = clamp(finite(track.volume, 0.8), 0.0, 2.0);
    track.pan = clamp(finite(track.pan, 0.0), -1.0, 1.0);
    track.delay_send = clamp(finite(track.delay_send, 0.0), 0.0, 1.0);
    track.reverb_send = clamp(finite(track.reverb_send, 0.0), 0.0, 1.0);
    track.instrument = clamp(track.instrument, 0, ctl.view().instruments.len() as i32 - 1);
    let p = ctl.edit();
    let index = index as usize;
    if p.tracks.len() <= index {
        p.tracks.resize(index + 1, default_track());
    }
    p.tracks[index] = track;
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_track_voice(
    e: *mut STEngine,
    track: i32,
    choke_group: i32,
    choke_scope: i32,
    voice_mode: i32,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if track < 0
        || track >= ST_TRACK_SANITY_LIMIT
        || choke_group < 0
        || choke_group > 127
        || choke_scope < 0
        || choke_scope > ST_TRACK_SANITY_LIMIT
        || voice_mode < -1
        || voice_mode > 2
    {
        return 0;
    }
    let p = control(e).edit();
    let track = track as usize;
    if p.track_voices.len() <= track {
        p.track_voices.resize(track + 1, TrackVoice::default());
    }
    p.track_voices[track] = TrackVoice { choke: choke_group, scope: choke_scope, mode: voice_mode };
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_host_mixing(e: *mut STEngine, enabled: i32, computational_mask: u32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    p.host_mixing = enabled != 0;
    p.computational_words.clear();
    p.computational_words.push(computational_mask as u64 | !0xffff_ffffu64);
    p.computational_beyond = true;
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
    let p = control(e).edit();
    p.host_mixing = enabled != 0;
    let count = cmin(word_count, ((ST_TRACK_SANITY_LIMIT + 63) / 64) as u32) as usize;
    p.computational_words.clear();
    if count != 0 {
        // SAFETY: `words` is non-null and holds `wordCount >= count` words (API contract).
        p.computational_words.extend_from_slice(unsafe { std::slice::from_raw_parts(words, count) });
    }
    p.computational_beyond = enabled_beyond != 0;
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_instrument_count(e: *mut STEngine, count: i32) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if count < 1 || count > ST_INSTRUMENT_SANITY_LIMIT {
        return 0;
    }
    control(e).edit().instruments.resize(count as usize, Instrument::default());
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_instrument(e: *mut STEngine, index: i32, instrument: STInstrument) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let ctl = control(e);
    if !ctl.view().has_instrument(index) {
        return;
    }
    let mut p = instrument;
    p.type_ = if p.type_ == ST_INSTRUMENT_AU || p.type_ == ST_INSTRUMENT_MIDI { p.type_ } else { ST_INSTRUMENT_SAMPLE };
    p.wavetable_position = clamp(finite(p.wavetable_position, 0.0), 0.0, 1.0);
    if p.wavetable_frame_size < ST_WAVETABLE_MIN_FRAME || p.wavetable_frame_size > ST_WAVETABLE_MAX_FRAME {
        p.wavetable_frame_size = 0;
    }
    p.voice_mode = clamp(p.voice_mode, 0, 2);
    p.choke_group = clamp(p.choke_group, 0, 127);
    p.mode = clamp(p.mode, 0, 6);
    p.root_note = clamp(p.root_note, 0, 127);
    p.filter_type = clamp(p.filter_type, 0, 2);
    p.slice_count = clamp(p.slice_count, 1, 128);
    p.bit_depth = clamp(p.bit_depth, 2, 16);
    p.volume = clamp(finite(p.volume, 0.7), 0.0, 2.0);
    p.pan = clamp(finite(p.pan, 0.0), -1.0, 1.0);
    p.tune = clamp(finite(p.tune, 0.0), -96.0, 96.0);
    p.start = clamp(finite(p.start, 0.0), 0.0, 0.999999);
    p.end = clamp(finite(p.end, 1.0), p.start + 0.000001, 1.0);
    p.loop_start = clamp(finite(p.loop_start, 0.0), 0.0, 0.999999);
    p.loop_end = clamp(finite(p.loop_end, 1.0), p.loop_start + 0.000001, 1.0);
    p.attack = clamp(finite(p.attack, 0.005), 0.0, 60.0);
    p.decay = clamp(finite(p.decay, 0.2), 0.0, 60.0);
    p.sustain = clamp(finite(p.sustain, 0.7), 0.0, 1.0);
    p.release = clamp(finite(p.release, 0.2), 0.0001, 60.0);
    p.cutoff = clamp(finite(p.cutoff, 18000.0), 20.0, 22000.0);
    p.resonance = clamp(finite(p.resonance, 0.0), 0.0, 0.99);
    p.drive = clamp(finite(p.drive, 0.0), 0.0, 1.0);
    p.lfo_rate = clamp(finite(p.lfo_rate, 0.0), 0.0, 100.0);
    p.lfo_depth = clamp(finite(p.lfo_depth, 0.0), 0.0, 1.0);
    p.grain_size = clamp(finite(p.grain_size, 0.08), 0.002, 0.5);
    p.grain_position = clamp(finite(p.grain_position, 0.5), 0.0, 1.0);
    p.grain_density = if p.grain_density >= 1 && p.grain_density <= ST_GRANULAR_MAX_GRAINS as i32 {
        p.grain_density
    } else {
        ST_GRANULAR_DEFAULT_DENSITY
    };
    p.grain_spray = clamp(finite(p.grain_spray, 0.0), 0.0, 1.0);
    p.grain_pitch_spread = clamp(finite(p.grain_pitch_spread, 0.0), 0.0, 24.0);
    p.grain_scan = clamp(finite(p.grain_scan, 0.0), -4.0, 4.0);
    p.grain_reverse = clamp(finite(p.grain_reverse, 0.0), 0.0, 1.0);
    p.grain_stereo = clamp(finite(p.grain_stereo, 0.0), 0.0, 1.0);
    p.filter_attack = clamp(finite(p.filter_attack, 0.005), 0.0, 60.0);
    p.filter_decay = clamp(finite(p.filter_decay, 0.2), 0.0, 60.0);
    p.filter_sustain = clamp(finite(p.filter_sustain, 0.0), 0.0, 1.0);
    p.filter_release = clamp(finite(p.filter_release, 0.2), 0.0001, 60.0);
    p.filter_env_amount = clamp(finite(p.filter_env_amount, 0.0), -48.0, 48.0);
    p.lfo_target = clamp(p.lfo_target, 0, 5);
    p.lfo2_target = clamp(p.lfo2_target, 0, 5);
    p.lfo_shape = clamp(p.lfo_shape, 0, 4);
    p.lfo2_shape = clamp(p.lfo2_shape, 0, 4);
    p.lfo2_rate = clamp(finite(p.lfo2_rate, 0.0), 0.0, 100.0);
    p.lfo2_depth = clamp(finite(p.lfo2_depth, 0.0), 0.0, 1.0);
    p.lfo_sync = (p.lfo_sync != 0) as i32;
    p.lfo2_sync = (p.lfo2_sync != 0) as i32;
    p.lfo_beats = clamp(finite(p.lfo_beats, 1.0), 1.0 / 64.0, 64.0);
    p.lfo2_beats = clamp(finite(p.lfo2_beats, 1.0), 1.0 / 64.0, 64.0);
    if let Some(slot) = ctl.edit().instruments.get_mut(index as usize) {
        slot.p = p;
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_pattern(e: *mut STEngine, index: i32, length: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    if index < 0 || index >= ST_MAX_PATTERNS {
        return;
    }
    let p = control(e).edit_patterns();
    let index = index as usize;
    if index >= p.patterns.len() {
        p.patterns.resize(index + 1, Pattern::default());
    }
    p.patterns[index].resize(clamp(length, 1, ST_MAX_ROWS));
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_reuse_pattern(e: *mut STEngine, index: i32) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let ctl = control(e);
    if index < 0 || index >= ST_MAX_PATTERNS {
        return 0;
    }
    let index = index as usize;
    let Some(published) = ctl.control.patterns.get(index).cloned() else { return 0 };
    let p = ctl.edit_patterns();
    if index >= p.patterns.len() {
        p.patterns.resize(index + 1, Pattern::default());
    }
    p.patterns[index] = published;
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_pattern_epoch(e: *mut STEngine) -> u64 {
    match unsafe { engine(e) } {
        Some(e) => control(e).control.pattern_epoch,
        None => 0,
    }
}

/// The step's note fields as the engine stores them (`st_engine_set_step`'s sanitizing).
fn sanitize_step_note(step: &mut STStep, instruments: usize) {
    step.instrument = clamp(step.instrument, -1, instruments as i32 - 1);
    step.velocity = clamp(step.velocity, 0, 127);
    step.gate = if finite(step.gate, 0.0) > 0.0 { clamp(step.gate, 0.01, 256.0) } else { 0.0 };
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    if pattern < 0 || track < 0 || track >= ST_TRACK_SANITY_LIMIT || row < 0 {
        return;
    }
    let p = control(e).edit_patterns();
    let instruments = p.instruments.len();
    let Some(target) = p.patterns.get_mut(pattern as usize) else { return };
    if row >= target.length {
        return;
    }
    let mut s = step;
    s.note = clamp(s.note, -3, 127);
    sanitize_step_note(&mut s, instruments);
    s.fx1.value = finite(s.fx1.value, 0.0);
    s.fx2.value = finite(s.fx2.value, 0.0);
    let v = target.mutable_step(track, row);
    v.plugin_fx = [PluginStepFX::NONE; 2];
    v.present = true;
    v.p = s;
    let has_chord = !chord.is_null() && chord_count > 0;
    v.count = if has_chord { clamp(chord_count, 1, ST_MAX_CHORD as i32) } else { 1 };
    v.chord = [0; ST_MAX_CHORD];
    if has_chord {
        // SAFETY: `chord` is non-null and holds `chordCount >= v.count` notes (API contract).
        let notes = unsafe { std::slice::from_raw_parts(chord, v.count as usize) };
        for (slot, &note) in v.chord.iter_mut().zip(notes) {
            *slot = clamp(note, -96, 96);
        }
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
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if pattern < 0
        || track < 0
        || track >= ST_TRACK_SANITY_LIMIT
        || row < 0
        || column < 0
        || column > 1
        || slot < -1
        || slot >= ST_MAX_INSERTS
        || host_destination < -1
        || host_destination >= ST_MAX_HOST_DESTINATIONS
        || !value.is_finite()
        || value < 0.0
        || value > 1.0
    {
        return 0;
    }
    let p = control(e).edit_patterns();
    let Some(target) = p.patterns.get_mut(pattern as usize) else { return 0 };
    if row >= target.length || !target.step(track, row).present {
        return 0;
    }
    target.mutable_step(track, row).plugin_fx[column as usize] =
        PluginStepFX { present: true, slot, destination: host_destination, address, value };
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_step(e: *mut STEngine, pattern: i32, track: i32, row: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    if pattern < 0 || track < 0 || track >= ST_TRACK_SANITY_LIMIT || row < 0 {
        return;
    }
    let p = control(e).edit_patterns();
    let Some(target) = p.patterns.get_mut(pattern as usize) else { return };
    if row >= target.length {
        return;
    }
    if target.has_column(track) {
        *target.mutable_step(track, row) = Step::BLANK;
    }
    for column in 1..=EXTRA_COLUMNS {
        if target.column_step(track, column, row).present {
            *target.mutable_column_step(track, column, row) = Step::BLANK;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_step_column(
    e: *mut STEngine,
    pattern: i32,
    track: i32,
    row: i32,
    column: i32,
    step: STStep,
) {
    let Some(engine_ref) = (unsafe { engine(e) }) else { return };
    if pattern < 0 || track < 0 || track >= ST_TRACK_SANITY_LIMIT || row < 0 || column < 0 || column > EXTRA_COLUMNS {
        return;
    }
    if column == 0 {
        // SAFETY: same engine handle, no chord.
        unsafe { st_engine_set_step(e, pattern, track, row, step, std::ptr::null(), 0) };
        return;
    }
    let p = control(engine_ref).edit_patterns();
    let instruments = p.instruments.len();
    let Some(target) = p.patterns.get_mut(pattern as usize) else { return };
    if row >= target.length {
        return;
    }
    let mut s = step;
    if s.note < -2 || s.note > 127 {
        if target.column_step(track, column, row).present {
            *target.mutable_column_step(track, column, row) = Step::BLANK;
        }
        return;
    }
    sanitize_step_note(&mut s, instruments);
    s.fx1 = STStepFX::default();
    s.fx2 = STStepFX::default();
    let v = target.mutable_column_step(track, column, row);
    *v = Step::BLANK;
    v.present = true;
    v.p = s;
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_automation(e: *mut STEngine, pattern: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    if pattern < 0 {
        return;
    }
    let p = control(e).edit_patterns();
    if let Some(target) = p.patterns.get_mut(pattern as usize) {
        target.lanes = None;
        target.host_lanes = None;
    }
}

/// Shared by native and host lanes: find or create the lane matching `key` (at most 256 lanes
/// per pattern) and append a point (at most 4096 per lane).
fn add_lane_point(target: &mut Pattern, host: bool, key: impl Fn(&Lane) -> bool, fresh: Lane, row: f64, value: f64) {
    {
        let shared = if host { target.host_lane_list() } else { target.lane_list() };
        let mut known = false;
        for lane in shared {
            if key(lane) {
                known = lane.points.len() < 4096;
                if !known {
                    return;
                }
                break;
            }
        }
        if !known && shared.len() >= 256 {
            return;
        }
    }
    let length = target.length as f64;
    let lanes = target.mutable_lanes(host);
    let index = match lanes.iter().position(&key) {
        Some(index) => index,
        None => {
            if lanes.len() >= 256 {
                return;
            }
            lanes.push(fresh);
            lanes.len() - 1
        }
    };
    let points = &mut lanes[index].points;
    if points.len() < 4096 {
        points.push(Point { row: clamp(row, 0.0, length), value });
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    if pattern < 0
        || track < 0
        || track >= st_channel_count(ST_TRACK_SANITY_LIMIT)
        || target < 0
        || target > ST_TARGET_LAST_INSTRUMENT
        || !row.is_finite()
        || !value.is_finite()
    {
        return;
    }
    let p = control(e).edit_patterns();
    let Some(pattern) = p.patterns.get_mut(pattern as usize) else { return };
    add_lane_point(
        pattern,
        false,
        |lane| lane.track == track && lane.target == target && lane.slot == slot && lane.address == address,
        Lane { track, target, slot, interpolation, address, points: Vec::new() },
        row,
        value,
    );
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    if pattern < 0 || destination < 0 || destination >= ST_MAX_HOST_DESTINATIONS || !row.is_finite() || !value.is_finite() {
        return;
    }
    let p = control(e).edit_patterns();
    let Some(pattern) = p.patterns.get_mut(pattern as usize) else { return };
    add_lane_point(
        pattern,
        true,
        |lane| lane.target == destination && lane.address == address,
        Lane {
            track: -1,
            target: destination,
            slot: 0,
            interpolation: if interpolation == 1 { 1 } else { 0 },
            address,
            points: Vec::new(),
        },
        row,
        value,
    );
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_host_automation_map(e: *mut STEngine, map: u64) {
    if let Some(e) = unsafe { engine(e) } {
        control(e).edit().host_map = map;
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    let last = p.patterns.len() as i32 - 1;
    p.sequence.clear();
    if !patterns.is_null() && count > 0 {
        // SAFETY: `patterns` is non-null and holds `count` entries (API contract).
        let entries = unsafe { std::slice::from_raw_parts(patterns, cmin(count, 4096) as usize) };
        p.sequence.extend(entries.iter().map(|&index| clamp(index, 0, last)));
    }
    if p.sequence.is_empty() {
        p.sequence.push(0);
    }
    p.loop_song = loop_song != 0;
    p.active_pattern = clamp(active_pattern, 0, last);
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    p.delay_time = clamp(finite(delay_seconds, 0.375), 0.001, 3.99);
    p.delay_feedback = clamp(finite(delay_feedback, 0.35), 0.0, 0.98);
    p.delay_mix = clamp(finite(delay_mix, 0.3), 0.0, 1.0);
    p.reverb_size = clamp(finite(reverb_size, 0.7), 0.0, 1.0);
    p.reverb_damping = clamp(finite(reverb_damping, 0.4), 0.0, 0.99);
    p.reverb_mix = clamp(finite(reverb_mix, 0.25), 0.0, 1.0);
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
    let ctl = control(e);
    if !ctl.view().has_instrument(instrument) {
        return 0;
    }
    let index = instrument as usize;
    if left.is_null() && frames == 0 {
        ctl.edit().instruments[index].set_sample(None);
        return publish_outside_update(e, ctl);
    }
    if !valid_pcm(left, frames, sample_rate) {
        return 0;
    }
    // SAFETY: `left` (and a non-null `right`) hold `frames` floats (API contract).
    let Some(sample) = (unsafe { make_sample(left, right, frames, sample_rate) }) else { return 0 };
    let reference = adopt_sample(ctl, sample);
    ctl.edit().instruments[index].set_sample(reference);
    publish_outside_update(e, ctl)
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_slice_points(e: *mut STEngine, instrument: i32, positions: *const f32, count: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let ctl = control(e);
    if !ctl.view().has_instrument(instrument) {
        return;
    }
    let slices = &mut ctl.edit().instruments[instrument as usize].slices;
    slices.clear();
    if !positions.is_null() && count > 0 {
        slices.push(0.0);
        // SAFETY: `positions` is non-null and holds `count` values (API contract).
        let values = unsafe { std::slice::from_raw_parts(positions, cmin(count, 128) as usize) };
        slices.extend(values.iter().copied().filter(|x| x.is_finite() && *x > 0.0 && *x < 1.0));
        slices.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        slices.dedup();
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_sample_cycle(e: *mut STEngine, instrument: i32, frames: i32) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let ctl = control(e);
    if !ctl.view().has_instrument(instrument)
        || frames < 0
        || (frames > 0 && (frames < ST_WAVETABLE_MIN_FRAME || frames > ST_WAVETABLE_MAX_FRAME))
    {
        return 0;
    }
    ctl.edit().instruments[instrument as usize].cycle_frames = frames;
    publish_outside_update(e, ctl)
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_zones(e: *mut STEngine, instrument: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let ctl = control(e);
    if ctl.view().has_instrument(instrument) {
        ctl.edit().instruments[instrument as usize].zones.clear();
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
    let ctl = control(e);
    if !ctl.view().has_instrument(instrument) || !valid_pcm(left, frames, sample_rate) {
        return 0;
    }
    let index = instrument as usize;
    if ctl.edit().instruments[index].zones.len() >= 128 {
        return 0;
    }
    // SAFETY: `left` (and a non-null `right`) hold `frames` floats (API contract).
    let Some(sample) = (unsafe { make_sample(left, right, frames, sample_rate) }) else { return 0 };
    let mut zone = Zone { sample: adopt_sample(ctl, sample), ..Zone::default() };
    zone.key_low = clamp(key_low, 0, 127);
    zone.key_high = clamp(key_high, zone.key_low, 127);
    zone.velocity_low = clamp(velocity_low, 0, 127);
    zone.velocity_high = clamp(velocity_high, zone.velocity_low, 127);
    zone.root_note = clamp(root_note, 0, 127);
    zone.tune = clamp(finite(tune, 0.0), -96.0, 96.0);
    zone.volume = clamp(finite(volume, 1.0), 0.0, 2.0);
    ctl.edit().instruments[index].zones.push(zone);
    publish_outside_update(e, ctl)
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
    if asset < 0 || asset >= AUDIO_ASSETS as i32 {
        return 0;
    }
    let ctl = control(e);
    let index = asset as usize;
    ctl.edit();
    if left.is_null() && frames == 0 {
        ctl.edit().audio_assets[index].sample = None;
        return publish_outside_update(e, ctl);
    }
    if !valid_pcm(left, frames, sample_rate) {
        return 0;
    }
    // SAFETY: `left` (and a non-null `right`) hold `frames` floats (API contract).
    let Some(sample) = (unsafe { make_sample(left, right, frames, sample_rate) }) else { return 0 };
    let reference = adopt_sample(ctl, sample);
    ctl.edit().audio_assets[index].sample = reference;
    publish_outside_update(e, ctl)
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_sample_zone_loop_enabled(
    e: *mut STEngine,
    instrument: i32,
    zone_index: i32,
    enabled: i32,
) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let ctl = control(e);
    if !ctl.view().has_instrument(instrument) || zone_index < 0 || enabled < -1 || enabled > 1 {
        return 0;
    }
    match ctl.edit().instruments[instrument as usize].zones.get_mut(zone_index as usize) {
        Some(zone) => {
            zone.loop_enabled = enabled;
            1
        }
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
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let ctl = control(e);
    if !ctl.view().has_instrument(instrument)
        || zone_index < 0
        || !start.is_finite()
        || !end.is_finite()
        || !loop_start.is_finite()
        || !loop_end.is_finite()
        || !pan.is_finite()
        || start < 0.0
        || end > 1.0
        || end - start < 0.000001
        || loop_start < 0.0
        || loop_end > 1.0
        || loop_end <= loop_start
        || pan < -1.0
        || pan > 1.0
    {
        return 0;
    }
    match ctl.edit().instruments[instrument as usize].zones.get_mut(zone_index as usize) {
        Some(zone) => {
            zone.start = start;
            zone.end = end;
            zone.loop_start = loop_start;
            zone.loop_end = loop_end;
            zone.pan = pan;
            zone.has_region = true;
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_clear_audio_clips(e: *mut STEngine) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    p.audio_clips.clear();
    p.pattern_clips.clear();
    p.song_tail_clips.clear();
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_add_song_tail_clip(e: *mut STEngine, asset: i32, track: i32, start_frame: i64, gain: f32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    if asset < 0 || asset >= AUDIO_ASSETS as i32 || track < 0 || track >= ST_TRACK_SANITY_LIMIT || start_frame < 0 {
        return;
    }
    let p = control(e).edit();
    if p.song_tail_clips.len() >= 64 {
        return;
    }
    p.song_tail_clips.push(SongTailClip { asset, track, start: start_frame, gain: clamp(finite(gain, 1.0), 0.0, 4.0) });
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    if head_asset < 0
        || head_asset >= AUDIO_ASSETS as i32
        || loop_asset >= AUDIO_ASSETS as i32
        || track < 0
        || track >= ST_TRACK_SANITY_LIMIT
        || pattern < 0
        || pattern >= ST_MAX_PATTERNS
    {
        return;
    }
    let p = control(e).edit();
    if p.pattern_clips.len() >= 256 {
        return;
    }
    p.pattern_clips.push(PatternClip {
        head_asset,
        loop_asset: if loop_asset < 0 { -1 } else { loop_asset },
        track,
        pattern,
        gain: clamp(finite(gain, 1.0), 0.0, 4.0),
    });
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    if asset < 0 || asset >= AUDIO_ASSETS as i32 || track < 0 || track >= ST_TRACK_SANITY_LIMIT {
        return;
    }
    let p = control(e).edit();
    if p.audio_clips.len() >= 256 {
        return;
    }
    p.audio_clips.push(AudioClip {
        asset,
        track,
        start_beat: clamp(finite64(start_beat, 0.0), 0.0, 1000000.0),
        duration_beats: clamp(finite64(duration_beats, 0.0), 0.0, 1000000.0),
        source_offset: clamp(finite64(source_offset, 0.0), 0.0, 1000000.0),
        gain: clamp(finite(gain, 1.0), 0.0, 4.0),
        fade_in: clamp(finite(fade_in, 0.005), 0.0, 3600.0),
        fade_out: clamp(finite(fade_out, 0.005), 0.0, 3600.0),
        muted: muted != 0,
        mapped_start: 0,
        mapped_end: 0,
    });
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
    unsafe { clear_error(error, error_capacity) };
    match control(e).commit(&e.shared, e.sample_rate) {
        Ok(()) => 1,
        Err(problem) => {
            unsafe { write_error(error, error_capacity, &problem) };
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_cancel_update(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        let ctl = control(e);
        ctl.draft = None;
        ctl.updating = false;
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
        e.shared.queue_transport(Command::new(CMD_PLAY, song_mode, 0, 0, 0, 0.0), true);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_song_loop(e: *mut STEngine, mut start_index: i32, mut end_index: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    if start_index < 0 || end_index < start_index {
        start_index = -1;
        end_index = -1;
    }
    p.song_loop_start = cmin(start_index, 4095);
    p.song_loop_end = cmin(end_index, 4095);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_send_effects(e: *mut STEngine, delay_beats: f64, delay_tone: f32, reverb_predelay: f32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let p = control(e).edit();
    p.delay_beats = if delay_beats.is_finite() && delay_beats > 0.0 { clamp(delay_beats, 1.0 / 64.0, 16.0) } else { 0.0 };
    let tone = clamp(finite(delay_tone, 1.0), 0.0, 1.0);
    // 1 = open; otherwise 200 Hz .. 20 kHz on a log scale.
    p.delay_tone_coef = if tone >= 1.0 {
        1.0
    } else {
        (1.0 - (-2.0 * std::f64::consts::PI * 200.0 * 100.0f64.powf(tone as f64) / e.sample_rate).exp()) as f32
    };
    p.reverb_predelay = clamp(finite(reverb_predelay, 0.0), 0.0, 0.5);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_pattern_loop(e: *mut STEngine, mut start_row: i32, mut end_row: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    if start_row < 0 || end_row < start_row {
        start_row = -1;
        end_row = -1;
    }
    e.shared.queue(&Command::new(
        CMD_PATTERN_LOOP,
        cmin(start_row, ST_MAX_ROWS - 1),
        cmin(end_row, ST_MAX_ROWS - 1),
        0,
        0,
        0.0,
    ));
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_stop(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue_transport(Command::new(CMD_STOP, 0, 0, 0, 0, 0.0), false);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_recording(e: *mut STEngine, enabled: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(CMD_RECORDING, (enabled != 0) as i32, 0, 0, 0, 0.0));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_seek(e: *mut STEngine, pattern: i32, row: i32, sequence: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(CMD_SEEK, pattern, row, sequence, 0, 0.0));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_queue_jump(e: *mut STEngine, pattern: i32, sequence: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(
            CMD_QUEUE_JUMP,
            if pattern < 0 { -1 } else { cmin(pattern, ST_MAX_PATTERNS - 1) },
            if sequence < 0 { -1 } else { cmin(sequence, 4095) },
            0,
            0,
            0.0,
        ));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_queue_mode(e: *mut STEngine, song_mode: i32, pattern: i32, sequence: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(
            CMD_QUEUE_MODE,
            (song_mode != 0) as i32,
            if pattern < 0 { -1 } else { cmin(pattern, ST_MAX_PATTERNS - 1) },
            if sequence < 0 { -1 } else { cmin(sequence, 4095) },
            0,
            0.0,
        ));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_play_count_in(e: *mut STEngine, song_mode: i32, bars: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue_transport(Command::new(CMD_COUNT_IN, (song_mode != 0) as i32, clamp(bars, 1, 4), 0, 0, 0.0), false);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_capture_transport(e: *mut STEngine) -> STTransportState {
    let Some(e) = (unsafe { engine(e) }) else { return STTransportState::default() };
    let r = render(e);
    r.flush_quiescent(&e.shared);
    // SAFETY: exclusive stopped-graph operation (API contract) standing in for the render
    // thread; the view ends with this call.
    unsafe { Rt::attach(r, &e.shared) }.capture_transport()
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_restore_transport(e: *mut STEngine, state: STTransportState, resume: i32) {
    let Some(e) = (unsafe { engine(e) }) else { return };
    let r = render(e);
    r.flush_quiescent(&e.shared);
    // SAFETY: as in `st_engine_capture_transport`.
    unsafe { Rt::attach(r, &e.shared) }.restore_transport(&state, resume != 0);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_note_on(e: *mut STEngine, track: i32, instrument: i32, note: i32, velocity: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(CMD_NOTE_ON, track, instrument, note, velocity, 0.0));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_note_off(e: *mut STEngine, track: i32, note: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(CMD_NOTE_OFF, track, note, 0, 0, 0.0));
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
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let ctl = control(e);
    if request_id == 0
        || request_id <= ctl.last_input_request_id
        || (edge != ST_INPUT_NOTE_ON && edge != ST_INPUT_NOTE_OFF)
        || track < 0
        || track >= ctl.control.channels()
        || !ctl.view().has_instrument(instrument)
        || note < 0
        || note > 127
        || velocity < 0
        || velocity > 127
        || (edge == ST_INPUT_NOTE_ON && velocity == 0)
        || (apply_note != 0 && apply_note != 1)
    {
        return 0;
    }
    let sh = &e.shared;
    let outstanding = sh.input_outstanding.fetch_add(1, Ordering::Relaxed);
    if outstanding >= ST_INPUT_ACK_CAPACITY as u32 - 1 {
        sh.input_outstanding.fetch_sub(1, Ordering::Relaxed);
        return 0;
    }
    let command = Command {
        kind: CMD_INPUT_NOTE,
        a: track,
        b: instrument,
        c: note,
        d: velocity,
        request_id,
        input_edge: edge,
        apply_input_note: apply_note,
        ..Command::default()
    };
    if !sh.commands.push(&command) {
        sh.input_outstanding.fetch_sub(1, Ordering::Relaxed);
        sh.dropped_commands.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    ctl.last_input_request_id = request_id;
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_pop_input_ack(e: *mut STEngine, ack: *mut STInputAck) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if ack.is_null() {
        return 0;
    }
    let Some(value) = e.shared.input_acks.pop() else { return 0 };
    // SAFETY: `ack` is a non-null pointer to caller storage for one STInputAck (API contract).
    unsafe { ack.write(value) };
    e.shared.input_outstanding.fetch_sub(1, Ordering::Relaxed);
    1
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
    let Some(e) = (unsafe { engine(e) }) else { return };
    let ctl = control(e);
    let mut command = Command { kind: CMD_PREVIEW, a: track, step, ..Command::default() };
    command.step.note = clamp(step.note, -3, 127);
    command.step.instrument = clamp(step.instrument, -1, ctl.view().instruments.len() as i32 - 1);
    command.step.velocity = clamp(step.velocity, 0, 127);
    command.value = clamp(finite(duration_seconds, 0.35), 0.03, 2.0);
    let has_chord = !chord.is_null() && chord_count > 0;
    command.chord_count = if has_chord { clamp(chord_count, 1, ST_MAX_CHORD as i32) } else { 1 };
    if has_chord {
        // SAFETY: `chord` is non-null and holds `chordCount >= chord_count` notes (API contract).
        let notes = unsafe { std::slice::from_raw_parts(chord, command.chord_count as usize) };
        for (slot, &note) in command.chord.iter_mut().zip(notes) {
            *slot = clamp(note, -96, 96);
        }
    }
    e.shared.queue(&command);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_panic(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(CMD_PANIC, 0, 0, 0, 0, 0.0));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_forget_external_voices(e: *mut STEngine, channel: i32) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    let r = render(e);
    // SAFETY: exclusive stopped-graph operation (API contract); `tb` points to the tables of
    // the project in use, which this thread may read while no render overlaps.
    let channels = unsafe { &*r.tb }.channels;
    if channel < 0 || channel >= channels {
        return 0;
    }
    let mut count = 0;
    let mut sink = VoiceSink { masks: &mut r.active_voice_masks, out: &r.out, sh: &e.shared };
    for (index, v) in r.voices.iter_mut().enumerate() {
        if v.active && v.external && v.track == channel {
            v.off_sent = true;
            sink.kill(v, index);
            count += 1;
        }
    }
    count
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_pitch_bend(e: *mut STEngine, track: i32, semitones: f32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(CMD_PITCH_BEND, track, 0, 0, 0, clamp(finite(semitones, 0.0), -48.0, 48.0)));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_mod_wheel(e: *mut STEngine, track: i32, value: f32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.queue(&Command::new(CMD_MOD_WHEEL, track, 0, 0, 0, clamp(finite(value, 0.0), 0.0, 1.0)));
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_set_instrument_parameter(e: *mut STEngine, instrument: i32, target: i32, value: f32) -> i32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0 };
    if instrument < 0 || instrument >= ST_INSTRUMENT_SANITY_LIMIT || !value.is_finite() {
        return 0;
    }
    if !(target == ST_TARGET_CUTOFF
        || target == ST_TARGET_RESONANCE
        || (target >= ST_TARGET_ATTACK && target <= ST_TARGET_LAST_INSTRUMENT))
    {
        return 0;
    }
    if e.shared.commands.available() == 0 {
        e.shared.dropped_commands.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    e.shared.queue(&Command::new(CMD_LIVE_PARAM, instrument, target, 0, 0, value));
    1
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_refresh_automation(e: *mut STEngine) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.automation_refresh_requested.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_render(e: *mut STEngine, left: *mut f32, right: *mut f32, frames: u32) {
    // SAFETY: the caller's buffers hold `frames` floats each (API contract).
    let (l, r) = unsafe { outputs(left, right, frames) };
    let Some(e) = (unsafe { engine(e) }) else {
        for out in [l, r].into_iter().flatten() {
            out.fill(0.0);
        }
        return;
    };
    let state = render(e);
    state.cache_valid = false;
    state.release_clock_origin();
    state.process(&e.shared, l, r, frames, false, 0, 0);
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
        for out in [l, r].into_iter().flatten() {
            out.fill(0.0);
        }
        return;
    };
    if frames == 0 {
        return;
    }
    render(e).render_cached(&e.shared, bus, channel, time, l, r, frames);
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_render_bus(
    e: *mut STEngine,
    bus: i32,
    sample_time: u64,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
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
        e.shared.cache_epoch.fetch_add(1, Ordering::Release);
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
    let mut s = STStatus::default();
    let Some(e) = (unsafe { engine(e) }) else { return s };
    let sh = &e.shared;
    let relaxed = Ordering::Relaxed;
    loop {
        let publication = sh.status_publication.load(Ordering::SeqCst);
        if publication & 1 != 0 {
            std::hint::spin_loop();
            continue;
        }
        let requested = sh.transport_requested.load(Ordering::Acquire);
        let applied = sh.status_transport_applied.load(Ordering::Acquire);
        s.transport_seek_revision = sh.status_seek_revision.load(relaxed);
        s.transport_pending = 0;
        s.requested_playing = 0;
        s.playing = sh.status_playing.load(relaxed);
        s.recording = sh.status_recording.load(relaxed);
        s.pattern = sh.status_pattern.load(relaxed);
        s.row = sh.status_row.load(relaxed);
        s.sequence = sh.status_sequence.load(relaxed);
        s.track_count = sh.status_tracks.load(relaxed);
        s.peak_l = sh.peak_l.load(relaxed);
        s.peak_r = sh.peak_r.load(relaxed);
        s.position = sh.status_position.load(relaxed);
        s.seconds = sh.status_seconds.load(relaxed);
        s.dropped_commands = sh.dropped_commands.load(relaxed);
        s.dropped_events = sh.dropped_events.load(relaxed);
        s.song_mode = sh.status_song_mode.load(relaxed);
        s.beat = sh.status_beat.load(relaxed);
        s.bpm = sh.status_bpm.load(relaxed);
        s.bar_start_beat = sh.status_bar_start.load(relaxed);
        s.bar = sh.status_bar.load(relaxed);
        s.numerator = sh.status_numerator.load(relaxed);
        s.denominator = sh.status_denominator.load(relaxed);
        s.timing_revision = sh.status_timing_revision.load(relaxed);
        s.queued_pattern = sh.status_queued_pattern.load(relaxed);
        s.queued_sequence = sh.status_queued_sequence.load(relaxed);
        s.count_in = sh.status_count_in.load(relaxed);
        // A queued Play/Stop/count-in not yet rendered: the rendered fields are stale until it
        // is; see st_status_transport_playing.
        if (requested.wrapping_sub(applied) as i32) > 0 {
            s.transport_pending = 1;
            s.requested_playing = sh.transport_requested_playing.load(relaxed);
        }
        fence(Ordering::SeqCst);
        if publication == sh.status_publication.load(Ordering::SeqCst) {
            return s;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_watch_voice_positions(e: *mut STEngine, instrument: i32) {
    if let Some(e) = unsafe { engine(e) } {
        e.shared.voice_watch.store(if instrument < -1 { -2 } else { instrument }, Ordering::Relaxed);
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
    let sh = &e.shared;
    for _ in 0..1000 {
        let before = sh.voice_seq.load(Ordering::Acquire);
        if before & 1 != 0 {
            continue;
        }
        let published_count = sh.voice_snapshot_count.load(Ordering::Relaxed);
        let count = clamp(cmin(capacity, published_count), 0, ST_VOICE_POSITION_CAPACITY as i32);
        // SAFETY: seqlock read. The copy goes straight to the caller's storage (`capacity >=
        // count` entries, API contract) without forming a reference to the snapshot, which the
        // render thread may be rewriting; a copy that raced with a write is rejected below
        // because `voice_seq` changed (or was odd), exactly as in the C++ engine.
        unsafe {
            std::ptr::copy_nonoverlapping(sh.voice_snapshot.get() as *const STVoicePosition, voices, count as usize);
        }
        let published = sh.voice_serial.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if sh.voice_seq.load(Ordering::Relaxed) == before {
            if !serial.is_null() {
                // SAFETY: `serial` is a non-null pointer to caller storage (API contract).
                unsafe { serial.write(published) };
            }
            return count;
        }
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_track_peak(e: *mut STEngine, track: i32) -> f32 {
    let Some(e) = (unsafe { engine(e) }) else { return 0.0 };
    if track < 0 {
        return 0.0;
    }
    let store = e.shared.peak_store.load(Ordering::Acquire);
    if store.is_null() {
        return 0.0;
    }
    // SAFETY: peak stores only grow and old ones live until the engine is destroyed, so a
    // pointer read from `peak_store` stays valid for any reader.
    unsafe { &*store }.load(track as usize)
}

#[no_mangle]
pub unsafe extern "C" fn st_engine_duration(e: *mut STEngine, song_mode: i32) -> f64 {
    let Some(e) = (unsafe { engine(e) }) else { return 0.0 };
    let p = &control(e).control;
    if song_mode != 0 {
        return p.song_duration(e.sample_rate);
    }
    if p.timing.is_some() {
        p.mapped_pattern_end.wrapping_sub(p.mapped_pattern_start) as f64 / e.sample_rate
    } else {
        p.pattern_rows(p.active_pattern) * 60.0 / p.bpm / p.lpb as f64
    }
}

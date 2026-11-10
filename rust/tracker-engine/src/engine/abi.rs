//! The C types and constants of `Sources/TrackerEngine/include/TrackerEngine.h`, with identical
//! layout. The clock structures the engine API exchanges (`STClockSpan`, `STClockTempoEvent`,
//! ...) are those of `crate::clock`, re-exported here.
//!
//! Field order and types follow the header exactly; fields are spelled in snake_case (the C name
//! is the same word in camelCase, e.g. `delay_send` for `delaySend`). The assertions at the end
//! of this file pin `size_of`/`offset_of!` of every structure to the values a C++ program prints
//! for the header.

pub use crate::clock::{
    STClockContext, STClockMeterEvent, STClockOrigin, STClockSpan, STClockTempoEvent, ST_CLOCK_BOUNDARY_BAR,
    ST_CLOCK_BOUNDARY_DOMAIN, ST_CLOCK_BOUNDARY_LOOP, ST_CLOCK_BOUNDARY_ORIGIN, ST_CLOCK_DOMAIN_END,
    ST_CLOCK_GRID_PROJECTION, ST_CLOCK_INVALID, ST_CLOCK_LOOPING, ST_CLOCK_MAX_SPAN, ST_CLOCK_OK,
    ST_CLOCK_OUTSIDE_DOMAIN, ST_CLOCK_PLAYING, ST_CLOCK_PREROLL, ST_CLOCK_RECORDING,
};

pub const ST_MIN_RETURN_BUS: i32 = 16;
pub const ST_TRACK_SANITY_LIMIT: i32 = 1024;
pub const ST_INSTRUMENT_SANITY_LIMIT: i32 = 1024;
pub const ST_MAX_NOTE_COLUMNS: i32 = 8;
pub const ST_MAX_PATTERNS: i32 = 256;
pub const ST_MAX_ROWS: i32 = 256;
pub const ST_MAX_CHORD: usize = 8;
pub const ST_MAX_HOST_DESTINATIONS: i32 = 2048;
pub const ST_MAX_INSERTS: i32 = 32;

pub const ST_INSTRUMENT_SAMPLE: i32 = 1;
pub const ST_INSTRUMENT_AU: i32 = 2;
pub const ST_INSTRUMENT_MIDI: i32 = 3;

pub const ST_WAVETABLE_DEFAULT_FRAME: i32 = 2048;
pub const ST_WAVETABLE_MIN_FRAME: i32 = 16;
pub const ST_WAVETABLE_MAX_FRAME: i32 = 8192;

pub const ST_MODE_ONESHOT: i32 = 0;
pub const ST_MODE_FORWARD: i32 = 1;
pub const ST_MODE_REVERSE: i32 = 2;
pub const ST_MODE_PINGPONG: i32 = 3;
pub const ST_MODE_SLICE: i32 = 4;
pub const ST_MODE_WAVETABLE: i32 = 5;
pub const ST_MODE_GRANULAR: i32 = 6;

pub const ST_FILTER_LOWPASS: i32 = 0;
pub const ST_FILTER_HIGHPASS: i32 = 1;
pub const ST_FILTER_BANDPASS: i32 = 2;

pub const ST_MOD_CUTOFF: i32 = 0;
pub const ST_MOD_PITCH: i32 = 1;
pub const ST_MOD_VOLUME: i32 = 2;
pub const ST_MOD_PAN: i32 = 3;
pub const ST_MOD_POSITION: i32 = 4;
pub const ST_MOD_GRAIN_SIZE: i32 = 5;

pub const ST_LFO_SINE: i32 = 0;
pub const ST_LFO_TRIANGLE: i32 = 1;
pub const ST_LFO_SAW: i32 = 2;
pub const ST_LFO_SQUARE: i32 = 3;
pub const ST_LFO_RANDOM: i32 = 4;

pub const ST_TARGET_VOLUME: i32 = 0;
pub const ST_TARGET_PAN: i32 = 1;
pub const ST_TARGET_CUTOFF: i32 = 2;
pub const ST_TARGET_RESONANCE: i32 = 3;
pub const ST_TARGET_DELAY: i32 = 4;
pub const ST_TARGET_REVERB: i32 = 5;
pub const ST_TARGET_PITCH: i32 = 6;
pub const ST_TARGET_PLUGIN: i32 = 7;
pub const ST_TARGET_ATTACK: i32 = 8;
pub const ST_TARGET_DECAY: i32 = 9;
pub const ST_TARGET_SUSTAIN: i32 = 10;
pub const ST_TARGET_RELEASE: i32 = 11;
pub const ST_TARGET_DRIVE: i32 = 12;
pub const ST_TARGET_BIT_DEPTH: i32 = 13;
pub const ST_TARGET_LFO_RATE: i32 = 14;
pub const ST_TARGET_LFO_DEPTH: i32 = 15;
pub const ST_TARGET_START: i32 = 16;
pub const ST_TARGET_END: i32 = 17;
pub const ST_TARGET_LOOP_START: i32 = 18;
pub const ST_TARGET_LOOP_END: i32 = 19;
pub const ST_TARGET_GRAIN_SIZE: i32 = 20;
pub const ST_TARGET_GRAIN_POSITION: i32 = 21;
pub const ST_TARGET_SLICE_COUNT: i32 = 22;
pub const ST_TARGET_INSTRUMENT_VOLUME: i32 = 23;
pub const ST_TARGET_INSTRUMENT_PAN: i32 = 24;
pub const ST_TARGET_TUNE: i32 = 25;
pub const ST_TARGET_FILTER_ATTACK: i32 = 26;
pub const ST_TARGET_FILTER_DECAY: i32 = 27;
pub const ST_TARGET_FILTER_SUSTAIN: i32 = 28;
pub const ST_TARGET_FILTER_RELEASE: i32 = 29;
pub const ST_TARGET_FILTER_ENV_AMOUNT: i32 = 30;
pub const ST_TARGET_LFO2_RATE: i32 = 31;
pub const ST_TARGET_LFO2_DEPTH: i32 = 32;
pub const ST_TARGET_LFO_BEATS: i32 = 33;
pub const ST_TARGET_LFO2_BEATS: i32 = 34;
pub const ST_TARGET_WAVETABLE_POSITION: i32 = 35;
pub const ST_TARGET_GRAIN_DENSITY: i32 = 36;
pub const ST_TARGET_GRAIN_SPRAY: i32 = 37;
pub const ST_TARGET_GRAIN_PITCH_SPREAD: i32 = 38;
pub const ST_TARGET_GRAIN_SCAN: i32 = 39;
pub const ST_TARGET_GRAIN_REVERSE: i32 = 40;
pub const ST_TARGET_GRAIN_STEREO: i32 = 41;
pub const ST_TARGET_LAST_INSTRUMENT: i32 = ST_TARGET_GRAIN_STEREO;

pub const ST_FX_NONE: i32 = 0;
pub const ST_FX_VOLUME: i32 = 1;
pub const ST_FX_PAN: i32 = 2;
pub const ST_FX_PITCH: i32 = 3;
pub const ST_FX_CUTOFF: i32 = 4;
pub const ST_FX_RESONANCE: i32 = 5;
pub const ST_FX_DELAY: i32 = 6;
pub const ST_FX_REVERB: i32 = 7;
pub const ST_FX_PROBABILITY: i32 = 8;
pub const ST_FX_RATCHET: i32 = 9;
pub const ST_FX_REVERSE: i32 = 10;
pub const ST_FX_OFFSET: i32 = 11;
pub const ST_FX_GLIDE: i32 = 12;
pub const ST_FX_MICROTIMING: i32 = 13;
pub const ST_FX_ARPEGGIO: i32 = 14;
pub const ST_FX_SLICE: i32 = 15;
pub const ST_FX_POSITION: i32 = 16;

pub const ST_EVENT_NOTE_ON: i32 = 1;
pub const ST_EVENT_NOTE_OFF: i32 = 2;
pub const ST_EVENT_PARAMETER: i32 = 3;
pub const ST_EVENT_ALL_NOTES_OFF: i32 = 4;
pub const ST_EVENT_HOST_PARAMETER: i32 = 5;

pub const ST_GRANULAR_MAX_GRAINS: usize = 8;
pub const ST_GRANULAR_DEFAULT_DENSITY: i32 = 2;

pub const ST_INPUT_NOTE_ON: i32 = 1;
pub const ST_INPUT_NOTE_OFF: i32 = 2;
pub const ST_INPUT_ACK_CAPACITY: usize = 4096;
pub const ST_INPUT_ACK_APPLIED: i32 = 1;
pub const ST_INPUT_ACK_DISCONTINUITY: i32 = 2;
pub const ST_INPUT_ACK_CONTROL_FLUSH: i32 = 3;
pub const ST_INPUT_CLOCK_PLAYING: i32 = 1;
pub const ST_INPUT_CLOCK_MAPPED: i32 = 2;
pub const ST_INPUT_CLOCK_RECORDING: i32 = 4;

/// `STFreezePhase` (a C enum: 32 bits).
pub type STFreezePhase = u32;
pub const ST_FREEZE_DISABLED: STFreezePhase = 0;
pub const ST_FREEZE_SONG: STFreezePhase = 1;
pub const ST_FREEZE_NATURAL_TAIL: STFreezePhase = 2;
pub const ST_FREEZE_STOPPED: STFreezePhase = 3;
pub const ST_FREEZE_UNSUPPORTED: STFreezePhase = 4;
pub const ST_FREEZE_INVALID_POSITION: STFreezePhase = 5;

/// `STFreezeCause` (a C enum: 32 bits).
pub type STFreezeCause = u32;
pub const ST_FREEZE_CAUSE_NONE: STFreezeCause = 0;
pub const ST_FREEZE_CAUSE_PLAY: STFreezeCause = 1;
pub const ST_FREEZE_CAUSE_SEEK: STFreezeCause = 2;
pub const ST_FREEZE_CAUSE_STOP: STFreezeCause = 3;
pub const ST_FREEZE_CAUSE_LOOP: STFreezeCause = 4;
pub const ST_FREEZE_CAUSE_NATURAL_END: STFreezeCause = 5;
pub const ST_FREEZE_CAUSE_REPLACEMENT: STFreezeCause = 6;
pub const ST_FREEZE_CAUSE_RESTORE: STFreezeCause = 7;
pub const ST_FREEZE_CAUSE_PANIC: STFreezeCause = 8;
pub const ST_FREEZE_CAUSE_GRAPH_RESET: STFreezeCause = 9;

pub const ST_FREEZE_PREROLL: u32 = 1;
pub const ST_FREEZE_NOT_CONFIGURED: i32 = -4;

pub const ST_ENGINE_CLOCK_NOT_PREPARED: i32 = -2;
pub const ST_ENGINE_CLOCK_NOT_RETAINED: i32 = -3;
pub const ST_ENGINE_CLOCK_HISTORY_FRAMES: usize = 262144;

pub const ST_VOICE_POSITION_CAPACITY: usize = 64;
pub const ST_VOICE_POSITION_RELEASING: i32 = 1;
pub const ST_VOICE_POSITION_PREVIEW: i32 = 2;
pub const ST_VOICE_POSITION_ZONE: i32 = 4;
pub const ST_VOICE_POSITION_SEQUENCED: i32 = 8;

/// `st_return_bus`: the stereo return/metronome bus follows the track buses, never below 16.
#[inline]
pub fn st_return_bus(track_count: i32) -> i32 {
    if track_count > ST_MIN_RETURN_BUS {
        track_count
    } else {
        ST_MIN_RETURN_BUS
    }
}
/// `st_channel_count`
#[inline]
pub fn st_channel_count(track_count: i32) -> i32 {
    st_return_bus(track_count)
}
/// `st_output_bus_count`
#[inline]
pub fn st_output_bus_count(track_count: i32) -> i32 {
    st_return_bus(track_count) + 1
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct STTrack {
    pub volume: f32,
    pub pan: f32,
    pub delay_send: f32,
    pub reverb_send: f32,
    pub mute: i32,
    pub solo: i32,
    pub instrument: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct STInstrument {
    pub type_: i32,
    pub mode: i32,
    pub root_note: i32,
    pub filter_type: i32,
    pub slice_count: i32,
    pub bit_depth: i32,
    pub volume: f32,
    pub pan: f32,
    pub tune: f32,
    pub start: f32,
    pub end: f32,
    pub loop_start: f32,
    pub loop_end: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub drive: f32,
    pub lfo_rate: f32,
    pub lfo_depth: f32,
    pub grain_size: f32,
    pub grain_position: f32,
    /// 0 poly, 1 mono, 2 legato.
    pub voice_mode: i32,
    /// 0 disables.
    pub choke_group: i32,
    pub filter_attack: f32,
    pub filter_decay: f32,
    pub filter_sustain: f32,
    pub filter_release: f32,
    pub filter_env_amount: f32,
    pub lfo_target: i32,
    pub lfo_shape: i32,
    pub lfo2_target: i32,
    pub lfo2_shape: i32,
    pub lfo2_rate: f32,
    pub lfo2_depth: f32,
    pub lfo_sync: i32,
    pub lfo2_sync: i32,
    pub lfo_beats: f32,
    pub lfo2_beats: f32,
    pub wavetable_position: f32,
    pub wavetable_frame_size: i32,
    pub grain_density: i32,
    pub grain_spray: f32,
    pub grain_pitch_spread: f32,
    pub grain_scan: f32,
    pub grain_reverse: f32,
    pub grain_stereo: f32,
}

impl STInstrument {
    /// C++ `STInstrument p{}`: every field zero.
    pub const ZERO: STInstrument = STInstrument {
        type_: 0,
        mode: 0,
        root_note: 0,
        filter_type: 0,
        slice_count: 0,
        bit_depth: 0,
        volume: 0.0,
        pan: 0.0,
        tune: 0.0,
        start: 0.0,
        end: 0.0,
        loop_start: 0.0,
        loop_end: 0.0,
        attack: 0.0,
        decay: 0.0,
        sustain: 0.0,
        release: 0.0,
        cutoff: 0.0,
        resonance: 0.0,
        drive: 0.0,
        lfo_rate: 0.0,
        lfo_depth: 0.0,
        grain_size: 0.0,
        grain_position: 0.0,
        voice_mode: 0,
        choke_group: 0,
        filter_attack: 0.0,
        filter_decay: 0.0,
        filter_sustain: 0.0,
        filter_release: 0.0,
        filter_env_amount: 0.0,
        lfo_target: 0,
        lfo_shape: 0,
        lfo2_target: 0,
        lfo2_shape: 0,
        lfo2_rate: 0.0,
        lfo2_depth: 0.0,
        lfo_sync: 0,
        lfo2_sync: 0,
        lfo_beats: 0.0,
        lfo2_beats: 0.0,
        wavetable_position: 0.0,
        wavetable_frame_size: 0,
        grain_density: 0,
        grain_spray: 0.0,
        grain_pitch_spread: 0.0,
        grain_scan: 0.0,
        grain_reverse: 0.0,
        grain_stereo: 0.0,
    };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STStepFX {
    pub type_: i32,
    pub value: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STStep {
    pub note: i32,
    pub instrument: i32,
    pub velocity: i32,
    pub gate: f32,
    pub fx1: STStepFX,
    pub fx2: STStepFX,
}

/// hostMap: on `ST_EVENT_HOST_PARAMETER`, the host destination map of the project that emitted
/// it; 0 on every other event.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STEvent {
    pub type_: i32,
    pub track: i32,
    pub instrument: i32,
    pub note: i32,
    pub velocity: i32,
    pub frame_offset: i32,
    pub target: i32,
    pub slot: i32,
    pub address: u64,
    pub value: f64,
    pub host_map: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STInputAck {
    pub request_id: u64,
    pub run_id: u64,
    pub timing_revision: u64,
    pub render_frame: i64,
    /// `INT64_MIN` for direct/non-bus render.
    pub graph_frame: i64,
    pub distance_beats: f64,
    pub edge: i32,
    pub track: i32,
    pub instrument: i32,
    pub note: i32,
    pub velocity: i32,
    pub apply_note: i32,
    pub status: i32,
    pub flags: i32,
    pub pattern: i32,
    pub row: i32,
    pub sequence: i32,
    pub early: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STFreezeContext {
    pub publication_id: u64,
    pub playback_id: u64,
    pub transition_id: u64,
    pub graph_frame: i64,
    pub content_graph_frame: i64,
    pub project_frame: i64,
    pub transition_content_graph_frame: i64,
    pub transition_project_frame: i64,
    pub sample_rate: f64,
    pub phase: STFreezePhase,
    pub cause: STFreezeCause,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STFreezeSpan {
    pub context: STFreezeContext,
    pub frames: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STStatus {
    pub playing: i32,
    pub pattern: i32,
    pub row: i32,
    pub sequence: i32,
    pub track_count: i32,
    pub peak_l: f32,
    pub peak_r: f32,
    pub position: f64,
    pub seconds: f64,
    pub dropped_commands: u64,
    pub dropped_events: u64,
    pub song_mode: i32,
    pub beat: f64,
    pub bpm: f64,
    pub bar_start_beat: f64,
    pub bar: i64,
    pub numerator: i32,
    pub denominator: i32,
    pub timing_revision: u64,
    pub recording: i32,
    /// Pending `st_engine_queue_jump` target, -1 none.
    pub queued_pattern: i32,
    pub queued_sequence: i32,
    /// Count-in clicks still to come (including the current one); 0 when not counting in.
    pub count_in: i32,
    pub transport_pending: i32,
    pub requested_playing: i32,
    /// Changes only when a seek is admitted; published coherently with position.
    pub transport_seek_revision: u64,
}

/// Ephemeral checkpoint for the same engine/project, not a saved project format.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STTransportState {
    pub playing: i32,
    pub song_mode: i32,
    pub pattern: i32,
    pub row: i32,
    pub sequence: i32,
    pub row_has_started: i32,
    pub sequence_finished: i32,
    pub row_remaining: f64,
    pub row_duration: f64,
    pub transport_frames: f64,
    pub timing_version: i32,
    pub timing_revision: u64,
    pub beat: f64,
    pub pattern_origin_beat: f64,
    pub loop_beat_offset: f64,
    pub loop_iteration: i64,
    pub recording: i32,
    pub pattern_clip_frame: i64,
    pub pattern_clip_pattern: i32,
    pub pattern_clip_looped: i32,
    pub pattern_clip_valid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STVoicePosition {
    pub instrument: i32,
    pub track: i32,
    pub note: i32,
    pub mode: i32,
    pub direction: i32,
    pub flags: i32,
    pub grain_count: i32,
    pub reserved: i32,
    pub position: f64,
    pub window_start: f64,
    pub window_end: f64,
    pub sample_frames: f64,
    pub level: f32,
    pub grain_levels: [f32; ST_GRANULAR_MAX_GRAINS],
    pub grain_positions: [f64; ST_GRANULAR_MAX_GRAINS],
}

/// `st_default_track`
pub const fn default_track() -> STTrack {
    STTrack { volume: 0.8, pan: 0.0, delay_send: 0.0, reverb_send: 0.0, mute: 0, solo: 0, instrument: 0 }
}

/// `st_default_instrument`
pub const fn default_instrument() -> STInstrument {
    let mut p = STInstrument::ZERO;
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.root_note = 60;
    p.slice_count = 16;
    p.bit_depth = 16;
    p.volume = 0.7;
    p.end = 1.0;
    p.loop_end = 1.0;
    p.attack = 0.005;
    p.decay = 0.2;
    p.sustain = 0.7;
    p.release = 0.2;
    p.cutoff = 18000.0;
    p.grain_size = 0.08;
    p.grain_position = 0.5;
    p.grain_density = ST_GRANULAR_DEFAULT_DENSITY;
    p.filter_attack = 0.005;
    p.filter_decay = 0.2;
    p.filter_sustain = 0.0;
    p.filter_release = 0.2;
    p.lfo_beats = 1.0;
    p.lfo2_beats = 1.0;
    p
}

/// `st_default_step`
pub const fn default_step() -> STStep {
    STStep {
        note: 60,
        instrument: -1,
        velocity: 100,
        gate: 0.8,
        fx1: STStepFX { type_: 0, value: 0.0 },
        fx2: STStepFX { type_: 0, value: 0.0 },
    }
}

impl Default for STTrack {
    fn default() -> Self {
        default_track()
    }
}
impl Default for STInstrument {
    fn default() -> Self {
        STInstrument::ZERO
    }
}

// Layout of every structure in TrackerEngine.h, as printed by a C++ program (clang++, x86_64) using
// sizeof/alignof/offsetof on the header; the same values hold on aarch64-apple-darwin.
const _: () = {
    use std::mem::{align_of, offset_of, size_of};

    assert!(size_of::<STTrack>() == 28 && align_of::<STTrack>() == 4);
    assert!(offset_of!(STTrack, volume) == 0);
    assert!(offset_of!(STTrack, pan) == 4);
    assert!(offset_of!(STTrack, delay_send) == 8);
    assert!(offset_of!(STTrack, reverb_send) == 12);
    assert!(offset_of!(STTrack, mute) == 16);
    assert!(offset_of!(STTrack, solo) == 20);
    assert!(offset_of!(STTrack, instrument) == 24);

    assert!(size_of::<STInstrument>() == 196 && align_of::<STInstrument>() == 4);
    assert!(offset_of!(STInstrument, type_) == 0);
    assert!(offset_of!(STInstrument, mode) == 4);
    assert!(offset_of!(STInstrument, root_note) == 8);
    assert!(offset_of!(STInstrument, filter_type) == 12);
    assert!(offset_of!(STInstrument, slice_count) == 16);
    assert!(offset_of!(STInstrument, bit_depth) == 20);
    assert!(offset_of!(STInstrument, volume) == 24);
    assert!(offset_of!(STInstrument, pan) == 28);
    assert!(offset_of!(STInstrument, tune) == 32);
    assert!(offset_of!(STInstrument, start) == 36);
    assert!(offset_of!(STInstrument, end) == 40);
    assert!(offset_of!(STInstrument, loop_start) == 44);
    assert!(offset_of!(STInstrument, loop_end) == 48);
    assert!(offset_of!(STInstrument, attack) == 52);
    assert!(offset_of!(STInstrument, decay) == 56);
    assert!(offset_of!(STInstrument, sustain) == 60);
    assert!(offset_of!(STInstrument, release) == 64);
    assert!(offset_of!(STInstrument, cutoff) == 68);
    assert!(offset_of!(STInstrument, resonance) == 72);
    assert!(offset_of!(STInstrument, drive) == 76);
    assert!(offset_of!(STInstrument, lfo_rate) == 80);
    assert!(offset_of!(STInstrument, lfo_depth) == 84);
    assert!(offset_of!(STInstrument, grain_size) == 88);
    assert!(offset_of!(STInstrument, grain_position) == 92);
    assert!(offset_of!(STInstrument, voice_mode) == 96);
    assert!(offset_of!(STInstrument, choke_group) == 100);
    assert!(offset_of!(STInstrument, filter_attack) == 104);
    assert!(offset_of!(STInstrument, filter_decay) == 108);
    assert!(offset_of!(STInstrument, filter_sustain) == 112);
    assert!(offset_of!(STInstrument, filter_release) == 116);
    assert!(offset_of!(STInstrument, filter_env_amount) == 120);
    assert!(offset_of!(STInstrument, lfo_target) == 124);
    assert!(offset_of!(STInstrument, lfo_shape) == 128);
    assert!(offset_of!(STInstrument, lfo2_target) == 132);
    assert!(offset_of!(STInstrument, lfo2_shape) == 136);
    assert!(offset_of!(STInstrument, lfo2_rate) == 140);
    assert!(offset_of!(STInstrument, lfo2_depth) == 144);
    assert!(offset_of!(STInstrument, lfo_sync) == 148);
    assert!(offset_of!(STInstrument, lfo2_sync) == 152);
    assert!(offset_of!(STInstrument, lfo_beats) == 156);
    assert!(offset_of!(STInstrument, lfo2_beats) == 160);
    assert!(offset_of!(STInstrument, wavetable_position) == 164);
    assert!(offset_of!(STInstrument, wavetable_frame_size) == 168);
    assert!(offset_of!(STInstrument, grain_density) == 172);
    assert!(offset_of!(STInstrument, grain_spray) == 176);
    assert!(offset_of!(STInstrument, grain_pitch_spread) == 180);
    assert!(offset_of!(STInstrument, grain_scan) == 184);
    assert!(offset_of!(STInstrument, grain_reverse) == 188);
    assert!(offset_of!(STInstrument, grain_stereo) == 192);

    assert!(size_of::<STStepFX>() == 8 && align_of::<STStepFX>() == 4);
    assert!(offset_of!(STStepFX, type_) == 0);
    assert!(offset_of!(STStepFX, value) == 4);

    assert!(size_of::<STStep>() == 32 && align_of::<STStep>() == 4);
    assert!(offset_of!(STStep, note) == 0);
    assert!(offset_of!(STStep, instrument) == 4);
    assert!(offset_of!(STStep, velocity) == 8);
    assert!(offset_of!(STStep, gate) == 12);
    assert!(offset_of!(STStep, fx1) == 16);
    assert!(offset_of!(STStep, fx2) == 24);

    assert!(size_of::<STEvent>() == 56 && align_of::<STEvent>() == 8);
    assert!(offset_of!(STEvent, type_) == 0);
    assert!(offset_of!(STEvent, track) == 4);
    assert!(offset_of!(STEvent, instrument) == 8);
    assert!(offset_of!(STEvent, note) == 12);
    assert!(offset_of!(STEvent, velocity) == 16);
    assert!(offset_of!(STEvent, frame_offset) == 20);
    assert!(offset_of!(STEvent, target) == 24);
    assert!(offset_of!(STEvent, slot) == 28);
    assert!(offset_of!(STEvent, address) == 32);
    assert!(offset_of!(STEvent, value) == 40);
    assert!(offset_of!(STEvent, host_map) == 48);

    assert!(size_of::<STInputAck>() == 96 && align_of::<STInputAck>() == 8);
    assert!(offset_of!(STInputAck, request_id) == 0);
    assert!(offset_of!(STInputAck, run_id) == 8);
    assert!(offset_of!(STInputAck, timing_revision) == 16);
    assert!(offset_of!(STInputAck, render_frame) == 24);
    assert!(offset_of!(STInputAck, graph_frame) == 32);
    assert!(offset_of!(STInputAck, distance_beats) == 40);
    assert!(offset_of!(STInputAck, edge) == 48);
    assert!(offset_of!(STInputAck, track) == 52);
    assert!(offset_of!(STInputAck, instrument) == 56);
    assert!(offset_of!(STInputAck, note) == 60);
    assert!(offset_of!(STInputAck, velocity) == 64);
    assert!(offset_of!(STInputAck, apply_note) == 68);
    assert!(offset_of!(STInputAck, status) == 72);
    assert!(offset_of!(STInputAck, flags) == 76);
    assert!(offset_of!(STInputAck, pattern) == 80);
    assert!(offset_of!(STInputAck, row) == 84);
    assert!(offset_of!(STInputAck, sequence) == 88);
    assert!(offset_of!(STInputAck, early) == 92);

    assert!(size_of::<STFreezeContext>() == 88 && align_of::<STFreezeContext>() == 8);
    assert!(offset_of!(STFreezeContext, publication_id) == 0);
    assert!(offset_of!(STFreezeContext, playback_id) == 8);
    assert!(offset_of!(STFreezeContext, transition_id) == 16);
    assert!(offset_of!(STFreezeContext, graph_frame) == 24);
    assert!(offset_of!(STFreezeContext, content_graph_frame) == 32);
    assert!(offset_of!(STFreezeContext, project_frame) == 40);
    assert!(offset_of!(STFreezeContext, transition_content_graph_frame) == 48);
    assert!(offset_of!(STFreezeContext, transition_project_frame) == 56);
    assert!(offset_of!(STFreezeContext, sample_rate) == 64);
    assert!(offset_of!(STFreezeContext, phase) == 72);
    assert!(offset_of!(STFreezeContext, cause) == 76);
    assert!(offset_of!(STFreezeContext, flags) == 80);

    assert!(size_of::<STFreezeSpan>() == 96 && align_of::<STFreezeSpan>() == 8);
    assert!(offset_of!(STFreezeSpan, context) == 0);
    assert!(offset_of!(STFreezeSpan, frames) == 88);

    assert!(size_of::<STStatus>() == 152 && align_of::<STStatus>() == 8);
    assert!(offset_of!(STStatus, playing) == 0);
    assert!(offset_of!(STStatus, pattern) == 4);
    assert!(offset_of!(STStatus, row) == 8);
    assert!(offset_of!(STStatus, sequence) == 12);
    assert!(offset_of!(STStatus, track_count) == 16);
    assert!(offset_of!(STStatus, peak_l) == 20);
    assert!(offset_of!(STStatus, peak_r) == 24);
    assert!(offset_of!(STStatus, position) == 32);
    assert!(offset_of!(STStatus, seconds) == 40);
    assert!(offset_of!(STStatus, dropped_commands) == 48);
    assert!(offset_of!(STStatus, dropped_events) == 56);
    assert!(offset_of!(STStatus, song_mode) == 64);
    assert!(offset_of!(STStatus, beat) == 72);
    assert!(offset_of!(STStatus, bpm) == 80);
    assert!(offset_of!(STStatus, bar_start_beat) == 88);
    assert!(offset_of!(STStatus, bar) == 96);
    assert!(offset_of!(STStatus, numerator) == 104);
    assert!(offset_of!(STStatus, denominator) == 108);
    assert!(offset_of!(STStatus, timing_revision) == 112);
    assert!(offset_of!(STStatus, recording) == 120);
    assert!(offset_of!(STStatus, queued_pattern) == 124);
    assert!(offset_of!(STStatus, queued_sequence) == 128);
    assert!(offset_of!(STStatus, count_in) == 132);
    assert!(offset_of!(STStatus, transport_pending) == 136);
    assert!(offset_of!(STStatus, requested_playing) == 140);
    assert!(offset_of!(STStatus, transport_seek_revision) == 144);

    assert!(size_of::<STTransportState>() == 136 && align_of::<STTransportState>() == 8);
    assert!(offset_of!(STTransportState, playing) == 0);
    assert!(offset_of!(STTransportState, song_mode) == 4);
    assert!(offset_of!(STTransportState, pattern) == 8);
    assert!(offset_of!(STTransportState, row) == 12);
    assert!(offset_of!(STTransportState, sequence) == 16);
    assert!(offset_of!(STTransportState, row_has_started) == 20);
    assert!(offset_of!(STTransportState, sequence_finished) == 24);
    assert!(offset_of!(STTransportState, row_remaining) == 32);
    assert!(offset_of!(STTransportState, row_duration) == 40);
    assert!(offset_of!(STTransportState, transport_frames) == 48);
    assert!(offset_of!(STTransportState, timing_version) == 56);
    assert!(offset_of!(STTransportState, timing_revision) == 64);
    assert!(offset_of!(STTransportState, beat) == 72);
    assert!(offset_of!(STTransportState, pattern_origin_beat) == 80);
    assert!(offset_of!(STTransportState, loop_beat_offset) == 88);
    assert!(offset_of!(STTransportState, loop_iteration) == 96);
    assert!(offset_of!(STTransportState, recording) == 104);
    assert!(offset_of!(STTransportState, pattern_clip_frame) == 112);
    assert!(offset_of!(STTransportState, pattern_clip_pattern) == 120);
    assert!(offset_of!(STTransportState, pattern_clip_looped) == 124);
    assert!(offset_of!(STTransportState, pattern_clip_valid) == 128);

    assert!(size_of::<STVoicePosition>() == 168 && align_of::<STVoicePosition>() == 8);
    assert!(offset_of!(STVoicePosition, instrument) == 0);
    assert!(offset_of!(STVoicePosition, track) == 4);
    assert!(offset_of!(STVoicePosition, note) == 8);
    assert!(offset_of!(STVoicePosition, mode) == 12);
    assert!(offset_of!(STVoicePosition, direction) == 16);
    assert!(offset_of!(STVoicePosition, flags) == 20);
    assert!(offset_of!(STVoicePosition, grain_count) == 24);
    assert!(offset_of!(STVoicePosition, reserved) == 28);
    assert!(offset_of!(STVoicePosition, position) == 32);
    assert!(offset_of!(STVoicePosition, window_start) == 40);
    assert!(offset_of!(STVoicePosition, window_end) == 48);
    assert!(offset_of!(STVoicePosition, sample_frames) == 56);
    assert!(offset_of!(STVoicePosition, level) == 64);
    assert!(offset_of!(STVoicePosition, grain_levels) == 68);
    assert!(offset_of!(STVoicePosition, grain_positions) == 104);
};

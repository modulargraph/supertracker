//! The plain data and constants of the engine API: tracks, instruments, steps, events, status,
//! and the bus arithmetic.
//!
//! The names carry the `ST` prefix: the mixer crate imports this module wholesale.

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
/// Bits of `STInputAck::flags`: the transport was playing / armed for recording at the edge.
pub const ST_INPUT_CLOCK_PLAYING: i32 = 1;
pub const ST_INPUT_CLOCK_RECORDING: i32 = 4;

pub const ST_VOICE_POSITION_CAPACITY: usize = 64;
pub const ST_VOICE_POSITION_RELEASING: i32 = 1;
pub const ST_VOICE_POSITION_PREVIEW: i32 = 2;
pub const ST_VOICE_POSITION_ZONE: i32 = 4;
pub const ST_VOICE_POSITION_SEQUENCED: i32 = 8;

/// The bus of the stereo return (the metronome and count-in clicks): it follows the track buses and is
/// never below [`ST_MIN_RETURN_BUS`].
#[inline]
pub const fn return_bus(track_count: i32) -> i32 {
    if track_count > ST_MIN_RETURN_BUS {
        track_count
    } else {
        ST_MIN_RETURN_BUS
    }
}

/// Logical channels of a project with `track_count` tracks: every channel below the return bus
/// can sound (tracks, and the unused channels up to the minimum return bus).
#[inline]
pub const fn channel_count(track_count: i32) -> i32 {
    return_bus(track_count)
}

/// Output buses of a project with `track_count` tracks: the channels and the return bus.
#[inline]
pub const fn output_bus_count(track_count: i32) -> i32 {
    return_bus(track_count) + 1
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STStepFX {
    pub type_: i32,
    pub value: f32,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STInputAck {
    pub request_id: u64,
    /// Changes whenever the transport's timeline does (Play, Stop, Seek, a tempo or arrangement
    /// change): `distance_beats` of two acknowledgements compare only within one run.
    pub run_id: u64,
    pub render_frame: i64,
    /// The host's frame of the edge: the `time` given to `render_block` plus the offset in the
    /// block. `i64::MIN` for a time beyond 2^52 frames.
    pub graph_frame: i64,
    /// Quarter beats played in this run up to the edge, counted through song loops.
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STStatus {
    pub playing: i32,
    pub pattern: i32,
    pub row: i32,
    pub sequence: i32,
    pub track_count: i32,
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
    pub recording: i32,
    /// Pending `queue_jump` target, -1 none.
    pub queued_pattern: i32,
    pub queued_sequence: i32,
    /// Count-in clicks still to come (including the current one); 0 when not counting in.
    pub count_in: i32,
    pub transport_pending: i32,
    pub requested_playing: i32,
    /// Changes only when a seek is admitted; published coherently with position.
    pub transport_seek_revision: u64,
}

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

impl STStatus {
    /// Whether the transport plays as far as the control side is concerned: a Play, Stop or
    /// count-in that the render half has not applied yet (`transport_pending`) already counts,
    /// so a caller that just asked for Play sees `true` before the next block is rendered.
    pub const fn transport_playing(&self) -> bool {
        (if self.transport_pending != 0 { self.requested_playing } else { self.playing }) != 0
    }

    /// The count-in beat to display: `count_in`, or 0 while a transport request is pending
    /// (the rendered value then belongs to the transport state being replaced).
    pub const fn count_in_shown(&self) -> i32 {
        if self.transport_pending != 0 {
            0
        } else {
            self.count_in
        }
    }
}

/// A track at unity: volume 0.8, centered, no sends, instrument 0.
pub const fn default_track() -> STTrack {
    STTrack { volume: 0.8, pan: 0.0, delay_send: 0.0, reverb_send: 0.0, mute: 0, solo: 0, instrument: 0 }
}

/// A sample instrument with neutral settings (see the fields for each default).
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

/// A step as the tracker enters one: middle C, no instrument, velocity 100, gate 0.8 rows, no effects.
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

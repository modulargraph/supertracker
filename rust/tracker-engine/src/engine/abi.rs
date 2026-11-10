//! The C types and constants of `Sources/TrackerEngine/include/TrackerEngine.h` (and the clock
//! structures of `TrackerClock.h` that the engine API exchanges), with identical layout.
//!
//! Field order and types follow the header exactly; `tests` in `engine/mod.rs` and the C++ layout
//! probe compare `size_of`/`offset_of!` of every structure against the C++ compiler.

#![allow(non_snake_case)]

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

// TrackerClock.h
pub const ST_CLOCK_OK: i32 = 1;
pub const ST_CLOCK_INVALID: i32 = 0;
pub const ST_CLOCK_OUTSIDE_DOMAIN: i32 = -1;
pub const ST_CLOCK_PLAYING: i32 = 1;
pub const ST_CLOCK_LOOPING: i32 = 2;
pub const ST_CLOCK_PREROLL: i32 = 4;
pub const ST_CLOCK_DOMAIN_END: i32 = 8;
pub const ST_CLOCK_RECORDING: i32 = 16;
pub const ST_CLOCK_GRID_PROJECTION: i32 = 32;
pub const ST_CLOCK_BOUNDARY_BAR: u32 = 4;
pub const ST_CLOCK_BOUNDARY_LOOP: u32 = 8;
pub const ST_CLOCK_BOUNDARY_ORIGIN: u32 = 16;
pub const ST_CLOCK_BOUNDARY_DOMAIN: u32 = 32;
pub const ST_CLOCK_MAX_SPAN: u32 = 65536;

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
    pub delaySend: f32,
    pub reverbSend: f32,
    pub mute: i32,
    pub solo: i32,
    pub instrument: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct STInstrument {
    pub type_: i32,
    pub mode: i32,
    pub rootNote: i32,
    pub filterType: i32,
    pub sliceCount: i32,
    pub bitDepth: i32,
    pub volume: f32,
    pub pan: f32,
    pub tune: f32,
    pub start: f32,
    pub end: f32,
    pub loopStart: f32,
    pub loopEnd: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub drive: f32,
    pub lfoRate: f32,
    pub lfoDepth: f32,
    pub grainSize: f32,
    pub grainPosition: f32,
    /// 0 poly, 1 mono, 2 legato.
    pub voiceMode: i32,
    /// 0 disables.
    pub chokeGroup: i32,
    pub filterAttack: f32,
    pub filterDecay: f32,
    pub filterSustain: f32,
    pub filterRelease: f32,
    pub filterEnvAmount: f32,
    pub lfoTarget: i32,
    pub lfoShape: i32,
    pub lfo2Target: i32,
    pub lfo2Shape: i32,
    pub lfo2Rate: f32,
    pub lfo2Depth: f32,
    pub lfoSync: i32,
    pub lfo2Sync: i32,
    pub lfoBeats: f32,
    pub lfo2Beats: f32,
    pub wavetablePosition: f32,
    pub wavetableFrameSize: i32,
    pub grainDensity: i32,
    pub grainSpray: f32,
    pub grainPitchSpread: f32,
    pub grainScan: f32,
    pub grainReverse: f32,
    pub grainStereo: f32,
}

impl STInstrument {
    /// C++ `STInstrument p{}`: every field zero.
    pub const ZERO: STInstrument = STInstrument {
        type_: 0,
        mode: 0,
        rootNote: 0,
        filterType: 0,
        sliceCount: 0,
        bitDepth: 0,
        volume: 0.0,
        pan: 0.0,
        tune: 0.0,
        start: 0.0,
        end: 0.0,
        loopStart: 0.0,
        loopEnd: 0.0,
        attack: 0.0,
        decay: 0.0,
        sustain: 0.0,
        release: 0.0,
        cutoff: 0.0,
        resonance: 0.0,
        drive: 0.0,
        lfoRate: 0.0,
        lfoDepth: 0.0,
        grainSize: 0.0,
        grainPosition: 0.0,
        voiceMode: 0,
        chokeGroup: 0,
        filterAttack: 0.0,
        filterDecay: 0.0,
        filterSustain: 0.0,
        filterRelease: 0.0,
        filterEnvAmount: 0.0,
        lfoTarget: 0,
        lfoShape: 0,
        lfo2Target: 0,
        lfo2Shape: 0,
        lfo2Rate: 0.0,
        lfo2Depth: 0.0,
        lfoSync: 0,
        lfo2Sync: 0,
        lfoBeats: 0.0,
        lfo2Beats: 0.0,
        wavetablePosition: 0.0,
        wavetableFrameSize: 0,
        grainDensity: 0,
        grainSpray: 0.0,
        grainPitchSpread: 0.0,
        grainScan: 0.0,
        grainReverse: 0.0,
        grainStereo: 0.0,
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
    pub frameOffset: i32,
    pub target: i32,
    pub slot: i32,
    pub address: u64,
    pub value: f64,
    pub hostMap: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STInputAck {
    pub requestID: u64,
    pub runID: u64,
    pub timingRevision: u64,
    pub renderFrame: i64,
    /// `INT64_MIN` for direct/non-bus render.
    pub graphFrame: i64,
    pub distanceBeats: f64,
    pub edge: i32,
    pub track: i32,
    pub instrument: i32,
    pub note: i32,
    pub velocity: i32,
    pub applyNote: i32,
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
    pub publicationID: u64,
    pub playbackID: u64,
    pub transitionID: u64,
    pub graphFrame: i64,
    pub contentGraphFrame: i64,
    pub projectFrame: i64,
    pub transitionContentGraphFrame: i64,
    pub transitionProjectFrame: i64,
    pub sampleRate: f64,
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
    pub trackCount: i32,
    pub peakL: f32,
    pub peakR: f32,
    pub position: f64,
    pub seconds: f64,
    pub droppedCommands: u64,
    pub droppedEvents: u64,
    pub songMode: i32,
    pub beat: f64,
    pub bpm: f64,
    pub barStartBeat: f64,
    pub bar: i64,
    pub numerator: i32,
    pub denominator: i32,
    pub timingRevision: u64,
    pub recording: i32,
    /// Pending `st_engine_queue_jump` target, -1 none.
    pub queuedPattern: i32,
    pub queuedSequence: i32,
    /// Count-in clicks still to come (including the current one); 0 when not counting in.
    pub countIn: i32,
    pub transportPending: i32,
    pub requestedPlaying: i32,
    /// Changes only when a seek is admitted; published coherently with position.
    pub transportSeekRevision: u64,
}

/// Ephemeral checkpoint for the same engine/project, not a saved project format.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STTransportState {
    pub playing: i32,
    pub songMode: i32,
    pub pattern: i32,
    pub row: i32,
    pub sequence: i32,
    pub rowHasStarted: i32,
    pub sequenceFinished: i32,
    pub rowRemaining: f64,
    pub rowDuration: f64,
    pub transportFrames: f64,
    pub timingVersion: i32,
    pub timingRevision: u64,
    pub beat: f64,
    pub patternOriginBeat: f64,
    pub loopBeatOffset: f64,
    pub loopIteration: i64,
    pub recording: i32,
    pub patternClipFrame: i64,
    pub patternClipPattern: i32,
    pub patternClipLooped: i32,
    pub patternClipValid: i32,
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
    pub grainCount: i32,
    pub reserved: i32,
    pub position: f64,
    pub windowStart: f64,
    pub windowEnd: f64,
    pub sampleFrames: f64,
    pub level: f32,
    pub grainLevels: [f32; ST_GRANULAR_MAX_GRAINS],
    pub grainPositions: [f64; ST_GRANULAR_MAX_GRAINS],
}

// ---- TrackerClock.h structures the engine API exchanges ----

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockTempoEvent {
    pub beat: f64,
    pub bpm: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockMeterEvent {
    pub beat: f64,
    pub numerator: i32,
    pub denominator: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockOrigin {
    pub graphOriginFrame: i64,
    pub startBeat: f64,
    pub playing: i32,
    pub loopEnabled: i32,
    pub loopStartBeat: f64,
    pub loopEndBeat: f64,
    pub epoch: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockContext {
    pub revision: u64,
    pub epoch: u64,
    pub graphFrame: i64,
    pub contentGraphFrame: i64,
    pub songFrame: i64,
    pub loopIteration: i64,
    pub bar: i64,
    pub sampleRate: f64,
    pub beat: f64,
    pub bpm: f64,
    pub barStartBeat: f64,
    pub barEndBeat: f64,
    pub numerator: i32,
    pub denominator: i32,
    pub flags: i32,
    /// Graph-frame coordinate; `INT64_MAX` when stopped.
    pub nextBoundaryFrame: i64,
    pub nextBoundaryKinds: u32,
    pub loopStartBeat: f64,
    pub loopEndBeat: f64,
    /// Graph coordinate; `INT64_MAX` if unreachable.
    pub nextQuarterBeatFrame: i64,
    pub nextQuarterGridFrame: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct STClockSpan {
    pub context: STClockContext,
    /// Positive except at the inclusive map endpoint.
    pub frames: u32,
}

/// `st_default_track`
pub const fn default_track() -> STTrack {
    STTrack { volume: 0.8, pan: 0.0, delaySend: 0.0, reverbSend: 0.0, mute: 0, solo: 0, instrument: 0 }
}

/// `st_default_instrument`
pub const fn default_instrument() -> STInstrument {
    let mut p = STInstrument::ZERO;
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.rootNote = 60;
    p.sliceCount = 16;
    p.bitDepth = 16;
    p.volume = 0.7;
    p.end = 1.0;
    p.loopEnd = 1.0;
    p.attack = 0.005;
    p.decay = 0.2;
    p.sustain = 0.7;
    p.release = 0.2;
    p.cutoff = 18000.0;
    p.grainSize = 0.08;
    p.grainPosition = 0.5;
    p.grainDensity = ST_GRANULAR_DEFAULT_DENSITY;
    p.filterAttack = 0.005;
    p.filterDecay = 0.2;
    p.filterSustain = 0.0;
    p.filterRelease = 0.2;
    p.lfoBeats = 1.0;
    p.lfo2Beats = 1.0;
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

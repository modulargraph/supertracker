//! The engine object's data, split by the
//! thread that owns them.
//!
//! * [`Shared`]: atomics and lock-free rings both threads touch.
//! * `Control` (see `control.rs`): the draft/published projects and everything the control
//!   thread allocates.
//! * [`Render`]: voices, transport and effects, touched only by the render thread.
//!
//! The render thread works through [`Rt`], a view that pairs `Render` with the project and the
//! channel tables it is currently using.

use super::abi::*;
use super::control::Control;
use super::project::{Project, Sample};
use super::tables::*;
use super::util::{boxed_slice, Ring};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU32, AtomicU64, Ordering};

pub const MAX_VOICES: usize = 256;
pub const MAX_EVENTS: usize = 65536;
pub const MAX_COMMANDS: usize = 2048;
pub const MAX_SCHEDULED: usize = 8192;
pub const VOICE_GROUPS: usize = MAX_VOICES / 64;
pub const SCHEDULED_GROUPS: usize = MAX_SCHEDULED / 64;

/// An `f64` published through an atomic (C++ `std::atomic<double>`).
pub struct AtomicF64(AtomicU64);
impl AtomicF64 {
    pub fn new(value: f64) -> Self {
        AtomicF64(AtomicU64::new(value.to_bits()))
    }
    #[inline(always)]
    pub fn load(&self, order: Ordering) -> f64 {
        f64::from_bits(self.0.load(order))
    }
    #[inline(always)]
    pub fn store(&self, value: f64, order: Ordering) {
        self.0.store(value.to_bits(), order)
    }
}

/// A queued control command (`Command`). `kind` values match the C++ `type` numbers.
#[derive(Clone, Copy)]
pub struct Command {
    pub kind: i32,
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
    pub value: f32,
    pub step: STStep,
    pub chord: [i32; ST_MAX_CHORD],
    pub chord_count: i32,
    pub request_id: u64,
    pub input_edge: i32,
    pub apply_input_note: i32,
}

impl Default for Command {
    fn default() -> Self {
        Command {
            kind: 0,
            a: 0,
            b: 0,
            c: 0,
            d: 0,
            value: 0.0,
            step: STStep::default(),
            chord: [0; ST_MAX_CHORD],
            chord_count: 1,
            request_id: 0,
            input_edge: 0,
            apply_input_note: 0,
        }
    }
}

impl Command {
    pub fn new(kind: i32, a: i32, b: i32, c: i32, d: i32, value: f32) -> Command {
        Command { kind, a, b, c, d, value, ..Command::default() }
    }
}

pub const CMD_PLAY: i32 = 1;
pub const CMD_STOP: i32 = 2;
pub const CMD_SEEK: i32 = 3;
pub const CMD_NOTE_ON: i32 = 4;
pub const CMD_NOTE_OFF: i32 = 5;
pub const CMD_PANIC: i32 = 6;
pub const CMD_PITCH_BEND: i32 = 7;
pub const CMD_PREVIEW: i32 = 8;
pub const CMD_RECORDING: i32 = 9;
pub const CMD_INPUT_NOTE: i32 = 10;
pub const CMD_PATTERN_LOOP: i32 = 11;
pub const CMD_QUEUE_JUMP: i32 = 12;
pub const CMD_COUNT_IN: i32 = 13;
pub const CMD_MOD_WHEEL: i32 = 14;
pub const CMD_LIVE_PARAM: i32 = 15;
pub const CMD_QUEUE_MODE: i32 = 16;

/// Granular mode: one slot per overlapping grain (window phase, random offset/pitch/direction/pan
/// drawn when the grain starts, last read position for the playhead snapshot).
#[derive(Clone, Copy)]
pub struct Grain {
    pub phase: f64,
    pub offset: f64,
    pub read: f64,
    pub ratio: f32,
    pub gain_l: f32,
    pub gain_r: f32,
    pub window: f32,
    pub dir: i32,
}

impl Grain {
    pub const NEW: Grain =
        Grain { phase: 2.0, offset: 0.0, read: 0.0, ratio: 1.0, gain_l: 1.0, gain_r: 1.0, window: 0.0, dir: 1 };
}

#[derive(Clone, Copy)]
pub struct Voice {
    /// Wavetable mode: frame width in samples and the smoothed table position.
    pub cycle_width: usize,
    pub position_smooth: f64,
    pub active: bool,
    pub releasing: bool,
    pub external: bool,
    pub off_sent: bool,
    pub sequenced: bool,
    pub preview: bool,
    pub explicit_slice: bool,
    pub track: i32,
    pub sequence_track: i32,
    pub instrument: i32,
    pub note: i32,
    pub age: i32,
    pub env_stage: i32,
    pub dir: i32,
    pub arp: i32,
    /// Choke group and its scope when the voice started (see `set_track_voice`).
    pub choke_group: i32,
    pub choke_scope: i32,
    // Cache only exact input matches; automation/modulation still evaluates each sample.
    pub frequency_pitch: f64,
    pub rate_pitch: f64,
    pub sample_increment: f64,
    pub frequency: f32,
    pub coefficient_cutoff: f32,
    pub coefficient_resonance: f32,
    pub filter_k: f32,
    pub filter_a1: f32,
    pub filter_a2: f32,
    pub filter_a3: f32,
    pub drive_amount: f32,
    pub drive_gain: f32,
    pub drive_scale: f32,
    // Region interpolation bounds change only with sample geometry.
    pub sample_first: usize,
    pub sample_last: usize,
    // A changed playhead/direction can switch loop crossfade or end-fade branches. Bridge that
    // discontinuity from the last emitted sample without moving either playhead or envelope.
    // Each voice owns its history, including overlapping tails.
    pub sample_history: bool,
    pub sample_transition: bool,
    pub sample_transition_left: i32,
    pub sample_transition_frames: i32,
    pub sample_last_l: f32,
    pub sample_last_r: f32,
    pub sample_correction_l: f32,
    pub sample_correction_r: f32,
    pub looped: bool,
    pub declick_in: bool,
    pub zone_region: bool,
    pub p: STInstrument,
    /// Counted by hand like the C++ voice: `note_on` takes a reference, `kill` returns it.
    pub sample: *const Sample,
    pub phase: f64,
    pub pos: f64,
    pub begin: f64,
    pub end: f64,
    pub loop_begin: f64,
    pub loop_end: f64,
    pub grain_phase: f64,
    pub gate: f64,
    pub onset_frame: f64,
    pub elapsed: f64,
    pub glide: f64,
    pub lfo_phase: f64,
    pub lfo2_phase: f64,
    pub lifetime: f64,
    pub filter_elapsed: f64,
    pub filter_env_stage: i32,
    pub filter_env: f32,
    pub filter_release_start: f32,
    pub lfo_random: f32,
    pub lfo2_random: f32,
    pub lfo_rng: u32,
    pub lfo2_rng: u32,
    pub velocity: f32,
    pub env: f32,
    pub release_start: f32,
    pub zone_tune: f32,
    pub zone_volume: f32,
    pub zone_pan: f32,
    pub zone_start: f32,
    pub zone_end: f32,
    pub volume_smooth: f32,
    pub pan_smooth: f32,
    pub cutoff_smooth: f32,
    pub zone_root: i32,
    pub filter_l1: f32,
    pub filter_l2: f32,
    pub filter_r1: f32,
    pub filter_r2: f32,
    pub rng: u32,
    /// Equal-power pan law for the last pan value (recomputed only when it changes).
    pub pan_law_pan: f32,
    pub pan_law_l: f32,
    pub pan_law_r: f32,
    pub grains: [Grain; ST_GRANULAR_MAX_GRAINS],
    pub grain_count: i32,
    pub grain_scan: f64,
    pub grain_center: f64,
    pub grain_span: f64,
}

impl Voice {
    /// C++ `Voice{}`.
    pub const NEW: Voice = Voice {
        cycle_width: ST_WAVETABLE_DEFAULT_FRAME as usize,
        position_smooth: 0.0,
        active: false,
        releasing: false,
        external: false,
        off_sent: false,
        sequenced: false,
        preview: false,
        explicit_slice: false,
        track: 0,
        sequence_track: -1,
        instrument: 0,
        note: 60,
        age: 0,
        env_stage: 0,
        dir: 1,
        arp: 0,
        choke_group: 0,
        choke_scope: -1,
        frequency_pitch: f64::NAN,
        rate_pitch: f64::NAN,
        sample_increment: 0.0,
        frequency: 0.0,
        coefficient_cutoff: -1.0,
        coefficient_resonance: -1.0,
        filter_k: 0.0,
        filter_a1: 0.0,
        filter_a2: 0.0,
        filter_a3: 0.0,
        drive_amount: -1.0,
        drive_gain: 1.0,
        drive_scale: 1.0,
        sample_first: 0,
        sample_last: 0,
        sample_history: false,
        sample_transition: false,
        sample_transition_left: 0,
        sample_transition_frames: 1,
        sample_last_l: 0.0,
        sample_last_r: 0.0,
        sample_correction_l: 0.0,
        sample_correction_r: 0.0,
        looped: false,
        declick_in: false,
        zone_region: false,
        p: STInstrument::ZERO,
        sample: std::ptr::null(),
        phase: 0.0,
        pos: 0.0,
        begin: 0.0,
        end: 0.0,
        loop_begin: 0.0,
        loop_end: 0.0,
        grain_phase: 0.0,
        gate: -1.0,
        onset_frame: 0.0,
        elapsed: 0.0,
        glide: 0.0,
        lfo_phase: 0.0,
        lfo2_phase: 0.0,
        lifetime: 0.0,
        filter_elapsed: 0.0,
        filter_env_stage: 0,
        filter_env: 0.0,
        filter_release_start: 0.0,
        lfo_random: 0.0,
        lfo2_random: 0.0,
        lfo_rng: 1,
        lfo2_rng: 1,
        velocity: 1.0,
        env: 0.0,
        release_start: 0.0,
        zone_tune: 0.0,
        zone_volume: 1.0,
        zone_pan: 0.0,
        zone_start: 0.0,
        zone_end: 1.0,
        volume_smooth: 0.7,
        pan_smooth: 0.0,
        cutoff_smooth: 18000.0,
        zone_root: -1,
        filter_l1: 0.0,
        filter_l2: 0.0,
        filter_r1: 0.0,
        filter_r2: 0.0,
        rng: 1,
        pan_law_pan: f32::NAN,
        pan_law_l: 1.0,
        pan_law_r: 1.0,
        grains: [Grain::NEW; ST_GRANULAR_MAX_GRAINS],
        grain_count: 0,
        grain_scan: 0.0,
        grain_center: 0.0,
        grain_span: 0.0,
    };
}

impl Default for Voice {
    fn default() -> Self {
        Voice::NEW
    }
}

/// A pending sequenced hit (delayed by microtiming or a ratchet).
#[derive(Clone, Copy)]
pub struct Scheduled {
    pub active: bool,
    pub due: f64,
    pub gate: f64,
    pub track: i32,
    pub instrument: i32,
    pub note: i32,
    pub velocity: i32,
    pub offset: f32,
    pub glide: f32,
    pub reverse: i32,
    pub arp: i32,
    pub slice: i32,
    pub sequence_track: i32,
}

impl Scheduled {
    pub const NEW: Scheduled = Scheduled {
        active: false,
        due: 0.0,
        gate: 0.0,
        track: 0,
        instrument: 0,
        note: 60,
        velocity: 100,
        offset: 0.0,
        glide: 0.0,
        reverse: -1,
        arp: 0,
        slice: -1,
        sequence_track: -1,
    };
}

impl Default for Scheduled {
    fn default() -> Self {
        Scheduled::NEW
    }
}

/// Reverb comb filter with a one-pole damping filter in the feedback path.
#[derive(Default)]
pub struct Comb {
    pub data: Vec<f32>,
    pub position: usize,
    pub filter: f32,
}

impl Comb {
    #[inline(always)]
    pub fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let Some(slot) = self.data.get_mut(self.position) else { return 0.0 };
        let y = *slot;
        self.filter = y * (1.0 - damp) + self.filter * damp;
        *slot = x + self.filter * feedback;
        self.position += 1;
        if self.position == self.data.len() {
            self.position = 0;
        }
        y
    }
    pub fn clear(&mut self) {
        self.data.fill(0.0);
        self.filter = 0.0;
        self.position = 0;
    }
}

#[derive(Default)]
pub struct Allpass {
    pub data: Vec<f32>,
    pub position: usize,
}

impl Allpass {
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let Some(slot) = self.data.get_mut(self.position) else { return 0.0 };
        let b = *slot;
        let y = b - x;
        *slot = x + b * 0.5;
        self.position += 1;
        if self.position == self.data.len() {
            self.position = 0;
        }
        y
    }
    pub fn clear(&mut self) {
        self.data.fill(0.0);
        self.position = 0;
    }
}

/// Live notes placed early on the next row (see `STInputAck.early`): that row's own trigger of
/// the note is skipped once.
#[derive(Clone, Copy)]
pub struct EarlyInput {
    pub track: i32,
    pub note: i32,
    pub pattern: i32,
    pub row: i32,
}

impl EarlyInput {
    pub const NONE: EarlyInput = EarlyInput { track: -1, note: -1, pattern: -1, row: -1 };
}

/// Printed tail of a pattern pass that handed over to another pattern.
#[derive(Clone, Copy, Default)]
pub struct PatternTail {
    pub active: bool,
    pub looped: bool,
    pub pattern: i32,
    pub position: i64,
}

#[derive(Clone, Copy, Default)]
pub struct SongTail {
    pub active: bool,
    pub clip: i32,
    pub position: i64,
}

/// Live instrument parameter (`set_instrument_parameter`): render-thread only, cleared
/// on publication.
#[derive(Clone, Copy)]
pub struct LiveParam {
    pub instrument: i32,
    pub target: i32,
    pub value: f32,
}

impl LiveParam {
    pub const NONE: LiveParam = LiveParam { instrument: -1, target: 0, value: 0.0 };
}

/// State both threads touch: atomics and single-producer/single-consumer rings.
pub struct Shared {
    /// Which engine this is: `Control` and `Render` only work with the `Shared` they were
    /// created with (see `host.rs`).
    pub(crate) engine_id: u64,
    /// The latest committed project not yet adopted by the render thread (owned `Box`).
    pub(crate) pending: AtomicPtr<Project>,
    /// Lock-free list of projects the render thread has replaced; freed by `collect()`.
    pub(crate) retired: AtomicPtr<Project>,
    pub(crate) commands: Ring<Command>,
    pub(crate) events: Ring<STEvent>,
    pub(crate) input_acks: Ring<STInputAck>,
    pub(crate) input_outstanding: AtomicU32,
    pub(crate) automation_refresh_requested: AtomicBool,
    pub(crate) status_playing: AtomicI32,
    pub(crate) status_recording: AtomicI32,
    pub(crate) status_pattern: AtomicI32,
    pub(crate) status_row: AtomicI32,
    pub(crate) status_sequence: AtomicI32,
    pub(crate) status_tracks: AtomicI32,
    pub(crate) status_song_mode: AtomicI32,
    pub(crate) status_queued_pattern: AtomicI32,
    pub(crate) status_queued_sequence: AtomicI32,
    pub(crate) status_count_in: AtomicI32,
    pub(crate) status_position: AtomicF64,
    pub(crate) status_seconds: AtomicF64,
    pub(crate) status_beat: AtomicF64,
    pub(crate) status_bpm: AtomicF64,
    pub(crate) status_bar_start: AtomicF64,
    pub(crate) status_bar: AtomicI64,
    pub(crate) status_numerator: AtomicI32,
    pub(crate) status_denominator: AtomicI32,
    pub(crate) dropped_commands: AtomicU64,
    pub(crate) dropped_events: AtomicU64,
    /// Voice playhead snapshot (`voice_positions`): seqlock, odd while the render
    /// thread writes.
    pub(crate) voice_watch: AtomicI32,
    pub(crate) voice_seq: AtomicU64,
    pub(crate) voice_snapshot_count: AtomicI32,
    pub(crate) voice_snapshot: UnsafeCell<[STVoicePosition; ST_VOICE_POSITION_CAPACITY]>,
    pub(crate) voice_serial: AtomicU64,
    /// Play/Stop/count-in are applied by the render thread, so the published status lags a
    /// queued transport command until the next render (or forever while no render runs). Each
    /// one carries a serial; until the render has applied and published the latest serial,
    /// `status` flags it pending together with the requested state.
    pub(crate) transport_requested: AtomicU32,
    pub(crate) status_transport_applied: AtomicU32,
    pub(crate) transport_requested_playing: AtomicI32,
    pub(crate) status_publication: AtomicU64,
    pub(crate) status_seek_revision: AtomicU64,
}

/// Source of process-unique engine identities.
static ENGINE_IDS: AtomicU64 = AtomicU64::new(0);

impl Shared {
    pub(crate) fn new() -> Shared {
        Shared {
            engine_id: ENGINE_IDS.fetch_add(1, Ordering::Relaxed) + 1,
            pending: AtomicPtr::new(std::ptr::null_mut()),
            retired: AtomicPtr::new(std::ptr::null_mut()),
            commands: Ring::new(MAX_COMMANDS),
            events: Ring::new(MAX_EVENTS),
            input_acks: Ring::new(ST_INPUT_ACK_CAPACITY),
            input_outstanding: AtomicU32::new(0),
            automation_refresh_requested: AtomicBool::new(false),
            status_playing: AtomicI32::new(0),
            status_recording: AtomicI32::new(0),
            status_pattern: AtomicI32::new(0),
            status_row: AtomicI32::new(0),
            status_sequence: AtomicI32::new(0),
            status_tracks: AtomicI32::new(8),
            status_song_mode: AtomicI32::new(0),
            status_queued_pattern: AtomicI32::new(-1),
            status_queued_sequence: AtomicI32::new(-1),
            status_count_in: AtomicI32::new(0),
            status_position: AtomicF64::new(0.0),
            status_seconds: AtomicF64::new(0.0),
            status_beat: AtomicF64::new(0.0),
            status_bpm: AtomicF64::new(120.0),
            status_bar_start: AtomicF64::new(0.0),
            status_bar: AtomicI64::new(1),
            status_numerator: AtomicI32::new(4),
            status_denominator: AtomicI32::new(4),
            dropped_commands: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            voice_watch: AtomicI32::new(-2),
            voice_seq: AtomicU64::new(0),
            voice_snapshot_count: AtomicI32::new(0),
            voice_snapshot: UnsafeCell::new([STVoicePosition::default(); ST_VOICE_POSITION_CAPACITY]),
            voice_serial: AtomicU64::new(0),
            transport_requested: AtomicU32::new(0),
            status_transport_applied: AtomicU32::new(0),
            transport_requested_playing: AtomicI32::new(0),
            status_publication: AtomicU64::new(0),
            status_seek_revision: AtomicU64::new(0),
        }
    }

    /// Control thread: queue a command, counting it as dropped when the ring is full.
    pub(crate) fn queue(&self, command: &Command) {
        if !self.commands.push(command) {
            self.dropped_commands.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Control thread: queue Play/Stop/count-in with a transport serial (see
    /// `transport_requested`).
    pub(crate) fn queue_transport(&self, mut command: Command, playing_after: bool) {
        let mut serial = self.transport_requested.load(Ordering::Relaxed).wrapping_add(1);
        if serial == 0 {
            serial = 1;
        }
        command.d = serial as i32;
        if !self.commands.push(&command) {
            self.dropped_commands.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.transport_requested_playing.store(playing_after as i32, Ordering::Relaxed);
        self.transport_requested.store(serial, Ordering::Release);
    }
}

impl Shared {
    /// A coherent snapshot of the published transport state (seqlock on
    /// `status_publication`). Any thread.
    pub fn status(&self) -> STStatus {
        let mut s = STStatus::default();
        let relaxed = Ordering::Relaxed;
        loop {
            let publication = self.status_publication.load(Ordering::SeqCst);
            if publication & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let requested = self.transport_requested.load(Ordering::Acquire);
            let applied = self.status_transport_applied.load(Ordering::Acquire);
            s.transport_seek_revision = self.status_seek_revision.load(relaxed);
            s.transport_pending = 0;
            s.requested_playing = 0;
            s.playing = self.status_playing.load(relaxed);
            s.recording = self.status_recording.load(relaxed);
            s.pattern = self.status_pattern.load(relaxed);
            s.row = self.status_row.load(relaxed);
            s.sequence = self.status_sequence.load(relaxed);
            s.track_count = self.status_tracks.load(relaxed);
            s.position = self.status_position.load(relaxed);
            s.seconds = self.status_seconds.load(relaxed);
            s.dropped_commands = self.dropped_commands.load(relaxed);
            s.dropped_events = self.dropped_events.load(relaxed);
            s.song_mode = self.status_song_mode.load(relaxed);
            s.beat = self.status_beat.load(relaxed);
            s.bpm = self.status_bpm.load(relaxed);
            s.bar_start_beat = self.status_bar_start.load(relaxed);
            s.bar = self.status_bar.load(relaxed);
            s.numerator = self.status_numerator.load(relaxed);
            s.denominator = self.status_denominator.load(relaxed);
            s.queued_pattern = self.status_queued_pattern.load(relaxed);
            s.queued_sequence = self.status_queued_sequence.load(relaxed);
            s.count_in = self.status_count_in.load(relaxed);
            // A queued Play/Stop/count-in not yet rendered: the rendered fields are stale until
            // it is; see `STStatus::transport_playing`.
            if (requested.wrapping_sub(applied) as i32) > 0 {
                s.transport_pending = 1;
                s.requested_playing = self.transport_requested_playing.load(relaxed);
            }
            std::sync::atomic::fence(Ordering::SeqCst);
            if publication == self.status_publication.load(Ordering::SeqCst) {
                return s;
            }
        }
    }

    /// Copy the latest playhead snapshot into `out` (up to its
    /// length). Returns the voice count and the snapshot serial, or `None` when no stable
    /// snapshot could be read. Any thread; never blocks the renderer.
    pub fn voice_positions(&self, out: &mut [STVoicePosition]) -> Option<(usize, u64)> {
        for _ in 0..1000 {
            let before = self.voice_seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let published = self.voice_snapshot_count.load(Ordering::Relaxed).clamp(0, ST_VOICE_POSITION_CAPACITY as i32);
            let count = out.len().min(published as usize);
            // SAFETY: seqlock read. The copy goes straight into the caller's storage without
            // forming a reference to the snapshot, which the render thread may be rewriting; a
            // copy that raced with a write is rejected below because `voice_seq` changed (or was
            // odd), exactly as in the C++ engine.
            unsafe {
                std::ptr::copy_nonoverlapping(self.voice_snapshot.get() as *const STVoicePosition, out.as_mut_ptr(), count);
            }
            let serial = self.voice_serial.load(Ordering::Relaxed);
            std::sync::atomic::fence(Ordering::Acquire);
            if self.voice_seq.load(Ordering::Relaxed) == before {
                return Some((count, serial));
            }
        }
        None
    }

    /// Instrument >= 0, -1 every instrument, less stops.
    pub fn watch_voice_positions(&self, instrument: i32) {
        self.voice_watch.store(if instrument < -1 { -2 } else { instrument }, Ordering::Relaxed);
    }

    /// Coalescing mailbox, callable from any thread.
    pub fn refresh_automation(&self) {
        self.automation_refresh_requested.store(true, Ordering::Release);
    }

    /// The next input acknowledgement. The ring has one consumer, the control half
    /// (`ControlHalf::pop_input_ack`), which is why this is not public.
    pub(crate) fn pop_input_ack(&self) -> Option<STInputAck> {
        let ack = self.input_acks.pop()?;
        self.input_outstanding.fetch_sub(1, Ordering::Relaxed);
        Some(ack)
    }

    /// The next engine event. The ring has one consumer, the render half
    /// (`RenderHalf::pop_event`), which is why this is not public.
    pub(crate) fn pop_event(&self) -> Option<STEvent> {
        self.events.pop()
    }
}

/// Where events go: the part of the render state `emit` needs, kept separate so voices can be
/// ended while other render fields are borrowed.
pub struct EventOut {
    pub(crate) frame_offset: i32,
}

/// `EngineCore::emit`. Returns false when the event was dropped.
///
/// Reserve one termination for every possible external voice. Automation and fresh notes may be
/// dropped under extreme pressure; accepted notes always retain enough room for note-off, so an
/// oversized AU pull cannot hang notes.
#[inline]
pub fn emit_event(out: &EventOut, sh: &Shared, mut event: STEvent) -> bool {
    event.frame_offset = if out.frame_offset > 0 { out.frame_offset } else { 0 };
    let free = sh.events.available();
    if ((event.type_ == ST_EVENT_PARAMETER || event.type_ == ST_EVENT_HOST_PARAMETER) && free <= (MAX_VOICES * 2) as u32)
        || (event.type_ == ST_EVENT_NOTE_ON && free <= MAX_VOICES as u32)
        || !sh.events.push(&event)
    {
        sh.dropped_events.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    true
}

/// Everything the render thread owns.
pub struct Render {
    /// The engine this render state belongs to (`Shared::engine_id`).
    pub(crate) engine_id: u64,
    pub(crate) sample_rate: f64,
    /// The project in use. Owned by the render thread from adoption (`pending`) until it pushes
    /// it onto `retired`; only `adopt_project` replaces it.
    pub(crate) current: *mut Project,
    /// Render view of `current->tables`; replaced only when a project is swapped in.
    pub(crate) tb: *mut TablesState,
    pub(crate) pending_input_acks: Box<[STInputAck]>,
    pub(crate) pending_input_count: usize,
    pub(crate) early_inputs: [EarlyInput; 16],
    pub(crate) early_input_next: usize,
    pub(crate) input_run_id: u64,
    pub(crate) input_loop_beats: f64,
    pub(crate) voices: Box<[Voice]>,
    pub(crate) active_voice_masks: [u64; VOICE_GROUPS],
    pub(crate) scheduled: Box<[Scheduled]>,
    pub(crate) scheduled_masks: [u64; SCHEDULED_GROUPS],
    /// Mask words that may hold pending notes: slots are taken lowest first, so the per-sample
    /// scan stops here.
    pub(crate) scheduled_groups_used: usize,
    pub(crate) lane_event_state: [LaneEventState; 256],
    pub(crate) host_lane_event_state: [LaneEventState; 256],
    pub(crate) automation_pattern: i32,
    pub(crate) plugin_row_lock_count: usize,
    pub(crate) plugin_locks_pending: i32,
    pub(crate) plugin_lane_locked: [bool; 256],
    pub(crate) plugin_host_lane_locked: [bool; 256],
    pub(crate) click_beat: i64,
    pub(crate) click_phase: f64,
    pub(crate) click_age: f64,
    pub(crate) click_accent: bool,
    pub(crate) delay_l: Vec<f32>,
    pub(crate) delay_r: Vec<f32>,
    pub(crate) delay_pos: usize,
    pub(crate) delay_tone_l: f32,
    pub(crate) delay_tone_r: f32,
    pub(crate) pre_l: Vec<f32>,
    pub(crate) pre_r: Vec<f32>,
    pub(crate) pre_pos: usize,
    pub(crate) comb_l: [Comb; 8],
    pub(crate) comb_r: [Comb; 8],
    pub(crate) ap_l: [Allpass; 4],
    pub(crate) ap_r: [Allpass; 4],
    pub(crate) playing: bool,
    pub(crate) recording: bool,
    pub(crate) song_mode: bool,
    pub(crate) row_has_started: bool,
    pub(crate) sequence_finished: bool,
    pub(crate) out: EventOut,
    pub(crate) song_end_frames: f64,
    pub(crate) scheduled_count: i32,
    pub(crate) pattern: i32,
    pub(crate) row: i32,
    pub(crate) sequence: i32,
    pub(crate) serial: i32,
    /// Optional pattern-mode loop range, inclusive rows; -1 disables.
    pub(crate) loop_start_row: i32,
    pub(crate) loop_end_row: i32,
    /// Optional jump honoured at the next pattern boundary; -1 none.
    pub(crate) queued_pattern: i32,
    pub(crate) queued_sequence: i32,
    /// With a queued jump: switch to Song (1) or Pattern (0) mode there; -1 keeps the mode.
    pub(crate) queued_mode: i32,
    /// Optional pre-roll before Play; 0 none.
    pub(crate) count_in_total: i64,
    pub(crate) count_in_elapsed: i64,
    pub(crate) count_in_click: i64,
    pub(crate) count_in_bar: i64,
    pub(crate) count_in_song: bool,
    /// Pattern-mode clip cursor: frames since the current pass's row 0, and printed tails of
    /// passes that handed over to another pattern.
    pub(crate) pattern_clip_frame: i64,
    pub(crate) pattern_clip_pattern: i32,
    pub(crate) pattern_clip_row: i32,
    pub(crate) pattern_clip_length: i32,
    pub(crate) pattern_clip_valid: bool,
    pub(crate) pattern_clip_looped: bool,
    pub(crate) pattern_tails: [PatternTail; 8],
    pub(crate) song_tails: [SongTail; 8],
    pub(crate) row_remaining: f64,
    pub(crate) row_duration: f64,
    pub(crate) transport_frames: f64,
    /// `transport_frames` (whole frames) at the first frame of the block rendered last; `None`
    /// when the transport did not play at that frame. See `RenderHalf::block_song_frame`.
    pub(crate) block_song_frame: Option<i64>,
    pub(crate) total_frames: f64,
    /// The musical clock of `transport_frames` as `Rt::update_clock` last derived it: the
    /// quarter beat, the bar (from 1) and the beat at which that bar starts.
    pub(crate) musical_beat: f64,
    pub(crate) bar: i64,
    pub(crate) bar_start_beat: f64,
    pub(crate) rng: u32,
    pub(crate) live_params: [LiveParam; 64],
    pub(crate) live_param_count: usize,
    pub(crate) voice_snapshot_empty: bool,
    pub(crate) voice_stage: [STVoicePosition; ST_VOICE_POSITION_CAPACITY],
    pub(crate) transport_applied: u32,
    pub(crate) seek_revision: u64,
    /// Raw host mixing of a block render: a channel's bus is exactly its summed input, so voices
    /// and clips add straight into the bus block (same order, same sums) instead of a per-sample
    /// channel pass.
    pub(crate) direct_bus: bool,
    pub(crate) direct_frame: u32,
    pub(crate) direct_frames: u32,
}

impl Render {
    /// Control thread, at engine creation. `current` is a fresh default project that shares the
    /// control project's tables.
    pub(crate) fn new(engine_id: u64, sample_rate: f64, current: *mut Project, tb: *mut TablesState) -> Render {
        let delay_length = (sample_rate * 4.0) as usize + 1;
        let pre_length = (sample_rate * 0.5) as usize + 1;
        const COMB_SIZES: [i32; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const ALLPASS_SIZES: [i32; 4] = [556, 441, 341, 225];
        let comb = |size: i32| Comb { data: vec![0.0; (size as f64 * sample_rate / 44100.0) as usize + 1], position: 0, filter: 0.0 };
        let allpass = |size: i32| Allpass { data: vec![0.0; (size as f64 * sample_rate / 44100.0) as usize + 1], position: 0 };
        Render {
            engine_id,
            sample_rate,
            current,
            tb,
            pending_input_acks: boxed_slice(MAX_COMMANDS),
            pending_input_count: 0,
            early_inputs: [EarlyInput::NONE; 16],
            early_input_next: 0,
            input_run_id: 1,
            input_loop_beats: 0.0,
            voices: boxed_slice(MAX_VOICES),
            active_voice_masks: [0; VOICE_GROUPS],
            scheduled: boxed_slice(MAX_SCHEDULED),
            scheduled_masks: [0; SCHEDULED_GROUPS],
            scheduled_groups_used: 0,
            lane_event_state: [LaneEventState::default(); 256],
            host_lane_event_state: [LaneEventState::default(); 256],
            automation_pattern: -1,
            plugin_row_lock_count: 0,
            plugin_locks_pending: 0,
            plugin_lane_locked: [false; 256],
            plugin_host_lane_locked: [false; 256],
            click_beat: -1,
            click_phase: 0.0,
            click_age: 1e9,
            click_accent: false,
            delay_l: vec![0.0; delay_length],
            delay_r: vec![0.0; delay_length],
            delay_pos: 0,
            delay_tone_l: 0.0,
            delay_tone_r: 0.0,
            pre_l: vec![0.0; pre_length],
            pre_r: vec![0.0; pre_length],
            pre_pos: 0,
            comb_l: COMB_SIZES.map(comb),
            comb_r: COMB_SIZES.map(|size| comb(size + 23)),
            ap_l: ALLPASS_SIZES.map(allpass),
            ap_r: ALLPASS_SIZES.map(|size| allpass(size + 23)),
            playing: false,
            recording: false,
            song_mode: false,
            row_has_started: false,
            sequence_finished: false,
            out: EventOut { frame_offset: 0 },
            song_end_frames: 0.0,
            scheduled_count: 0,
            pattern: 0,
            row: 0,
            sequence: 0,
            serial: 0,
            loop_start_row: -1,
            loop_end_row: -1,
            queued_pattern: -1,
            queued_sequence: -1,
            queued_mode: -1,
            count_in_total: 0,
            count_in_elapsed: 0,
            count_in_click: 1,
            count_in_bar: 1,
            count_in_song: false,
            pattern_clip_frame: 0,
            pattern_clip_pattern: -1,
            pattern_clip_row: -1,
            pattern_clip_length: 0,
            pattern_clip_valid: false,
            pattern_clip_looped: false,
            pattern_tails: [PatternTail::default(); 8],
            song_tails: [SongTail::default(); 8],
            row_remaining: 0.0,
            row_duration: 6000.0,
            transport_frames: 0.0,
            block_song_frame: None,
            total_frames: 0.0,
            musical_beat: 0.0,
            bar: 1,
            bar_start_beat: 0.0,
            rng: 0xa173cc79,
            live_params: [LiveParam::NONE; 64],
            live_param_count: 0,
            voice_snapshot_empty: true,
            voice_stage: [STVoicePosition::default(); ST_VOICE_POSITION_CAPACITY],
            transport_applied: 0,
            seek_revision: 0,
            direct_bus: false,
            direct_frame: 0,
            direct_frames: 0,
        }
    }
}

/// The render thread at work: `Render` together with the project and channel tables in use.
///
/// The render thread's work is written as methods of this view (in `transport.rs`,
/// `sequencer.rs`, `voice.rs` and `mixer.rs`).
pub struct Rt<'a> {
    pub(crate) e: &'a mut Render,
    pub(crate) p: &'a Project,
    pub(crate) tb: &'a mut TablesState,
    pub(crate) sh: &'a Shared,
}

impl<'a> Rt<'a> {
    /// Pair the render state with the project and tables it currently uses.
    ///
    /// # Safety
    /// The caller must be the render thread (or engine creation and destruction, when no render
    /// thread exists), and the returned view must be dropped before the next `Render::refresh`
    /// (which may retire the project it borrows).
    #[inline(always)]
    pub unsafe fn attach(e: &'a mut Render, sh: &'a Shared) -> Rt<'a> {
        // SAFETY: `current` always points to a live project: the render thread received it
        // through `pending` (or at creation) and it is freed only by `collect()` after
        // `adopt_project` has pushed it onto `retired`, which cannot happen while this view
        // exists (see this function's contract). Published projects are never mutated.
        let p: &'a Project = unsafe { &*e.current };
        // SAFETY: `tb` points into the `ChannelTables` kept alive by `current.tables`; its state
        // is touched by the render thread only, and `e` is borrowed exclusively, so this is the
        // only live reference to it.
        let tb: &'a mut TablesState = unsafe { &mut *e.tb };
        Rt { e, p, tb, sh }
    }
}

/// The engine's storage: what both halves share, and the state each half owns. Private to the
/// crate; a host reaches it through the handles of `host.rs`, where the reasons for its
/// `Send`/`Sync` implementations are written down.
pub(crate) struct EngineCore {
    pub(crate) sample_rate: f64,
    pub(crate) shared: Shared,
    /// Control-thread state. Access rule: dereferenced only through the engine's one
    /// `ControlHalf`, borrowed exclusively.
    pub(crate) control: UnsafeCell<Control>,
    /// Render-thread state. Access rule: dereferenced only through the engine's one
    /// `RenderHalf`, borrowed exclusively.
    pub(crate) render: UnsafeCell<Render>,
}

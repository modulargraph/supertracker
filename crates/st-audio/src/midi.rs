//! The MIDI seam of the audio core: what the render thread tells MIDI output, block by block.
//!
//! `st-audio` does not know MIDI devices. The host gives the engine a [`MidiOutput`]
//! ([`crate::AudioEngine::set_midi_output`]); every renderer the engine makes (one per engine
//! instance: a restart at another sample rate makes a new one) asks it for a [`MidiRender`], and
//! calls that on the render thread:
//!
//! 1. [`MidiRender::begin_block`] once per rendered block, with the graph time of its first
//!    frame and the transport position there (for MIDI clock and Start/Stop/Continue);
//! 2. [`MidiRender::note`] for every note event of the engine (the engine emits them for the
//!    instruments it does not sound itself: external MIDI instruments and plug-ins). The answer
//!    tells whether the note belonged to an external MIDI instrument;
//! 3. [`MidiRender::all_notes_off`] when the transport stops and when everything is silenced
//!    (panic).
//!
//! `st-host` implements both traits over `st_midi::RenderOutput` (`st_host::areas::midi`).
//!
//! # Real-time contract
//!
//! Everything a [`MidiRender`] does is called from [`crate::graph::render::Renderer::render`]:
//! no allocation, no lock, no system call other than reading a clock. [`RenderMidi`] itself
//! only reads the engine's status (atomics).
//!
//! # Offline renders
//!
//! [`crate::AudioEngine::render_offline`] suspends the hook for its duration
//! ([`RenderMidi::suspend`]): an export never plays external instruments or sends clock. An
//! engine made for offline work (no [`MidiOutput`] set) has no hook at all.
//!
//! # When a block is heard
//!
//! [`MidiBlock::playback_delay`] is what the device says lies between the callback and the
//! sound. The device layer sets it before every block ([`RenderMidi::set_playback_delay`], from
//! `cpal`'s callback timestamps; zero on the null device), and the MIDI layer schedules a
//! block's messages that much later, so external instruments sound with the engine's own
//! output. Not verified on a real device: how accurate the reported delay is depends on the
//! driver.

use std::sync::Arc;
use std::time::Duration;

/// The controllers that are pedals: sustain, sostenuto and soft. An instrument that was told
/// one is down must be told when it comes up, so everything that silences an instrument sends
/// them at zero.
pub const PEDAL_CONTROLLERS: [u8; 3] = [64, 66, 67];

use tracker_engine::engine::abi::{STEvent, ST_EVENT_NOTE_OFF, ST_EVENT_NOTE_ON};
use tracker_engine::engine::host::StatusHandle;

/// One render block as MIDI output needs to know it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiBlock {
    /// Graph time of the block's first frame (frames rendered since the renderer was made).
    pub time: u64,
    /// Frames in the block.
    pub frames: u32,
    pub sample_rate: f64,
    /// Transport position at the block's first frame, in quarter notes.
    pub beat: f64,
    /// The song plays.
    pub playing: bool,
    /// Tempo in beats per minute.
    pub tempo: f64,
    /// How long after the render call the block's first frame is heard (the device's output
    /// latency); zero while the device layer does not report it.
    pub playback_delay: Duration,
}

/// MIDI output as the render thread drives it. See the module documentation for the order of
/// calls and the real-time contract.
pub trait MidiRender: Send {
    /// A block is about to be mixed; note events of this block follow.
    fn begin_block(&mut self, block: &MidiBlock);
    /// A note of `instrument` at `frame_offset` frames into the block. Returns true when the
    /// instrument is an external MIDI instrument (the note went to its device).
    fn note(&mut self, instrument: usize, on: bool, note: u8, velocity: u8, frame_offset: i32) -> bool;
    /// Silence every external instrument now.
    fn all_notes_off(&mut self);
}

/// Where renderers get their [`MidiRender`] from. Implemented by the host over its MIDI service.
pub trait MidiOutput: Send + Sync {
    /// A handle for one renderer (one audio stream). Called on a control thread.
    fn render_hook(&self) -> Box<dyn MidiRender>;
}

/// The renderer's MIDI state: the hook, and what it needs to remember between blocks.
#[derive(Default)]
pub struct RenderMidi {
    hook: Option<Box<dyn MidiRender>>,
    suspended: bool,
    was_playing: bool,
    playback_delay: Duration,
}

impl RenderMidi {
    /// Sets or removes the hook (control thread, with the renderer locked).
    pub fn set_hook(&mut self, hook: Option<Box<dyn MidiRender>>) {
        self.hook = hook;
        self.was_playing = false;
    }

    /// True when a hook is installed.
    pub fn is_connected(&self) -> bool {
        self.hook.is_some()
    }

    /// While suspended nothing reaches the hook (offline renders). Returns the previous value.
    pub fn suspend(&mut self, suspended: bool) -> bool {
        std::mem::replace(&mut self.suspended, suspended)
    }

    /// The device's output latency for the blocks that follow (device callback, before `render`).
    pub fn set_playback_delay(&mut self, delay: Duration) {
        self.playback_delay = delay;
    }

    /// Render thread: a block of `frames` frames starting at graph time `time` was just rendered
    /// by the engine (its status is the position at the block's end).
    #[inline]
    pub(crate) fn begin_block(&mut self, status: &StatusHandle, time: u64, frames: usize, sample_rate: f64) {
        if self.suspended {
            return;
        }
        let Some(hook) = self.hook.as_mut() else { return };
        let engine = status.status();
        let playing = engine.playing != 0 && engine.count_in == 0;
        let tempo = if engine.bpm.is_finite() && engine.bpm > 0.0 { engine.bpm } else { 120.0 };
        // The status is the position after this block; the clock wants the position at its start.
        let advanced = if playing && sample_rate > 0.0 { frames as f64 / sample_rate * tempo / 60.0 } else { 0.0 };
        let beat = if engine.beat.is_finite() { (engine.beat - advanced).max(0.0) } else { 0.0 };
        hook.begin_block(&MidiBlock { time, frames: frames as u32, sample_rate, beat, playing, tempo, playback_delay: self.playback_delay });
        if self.was_playing && !playing {
            // The engine ends its external voices itself (their note-offs follow as events); this
            // also releases pedals and whatever a device still holds.
            hook.all_notes_off();
        }
        self.was_playing = playing;
    }

    /// Render thread: one event of the engine. Returns true when it was a note of an external
    /// MIDI instrument.
    #[inline]
    pub(crate) fn event(&mut self, event: &STEvent) -> bool {
        if self.suspended || (event.type_ != ST_EVENT_NOTE_ON && event.type_ != ST_EVENT_NOTE_OFF) || event.instrument < 0 {
            return false;
        }
        let Some(hook) = self.hook.as_mut() else { return false };
        let on = event.type_ == ST_EVENT_NOTE_ON;
        hook.note(event.instrument as usize, on, event.note.clamp(0, 127) as u8, event.velocity.clamp(0, 127) as u8, event.frame_offset)
    }

    /// Render thread: everything was silenced (panic).
    #[inline]
    pub(crate) fn silence(&mut self) {
        if self.suspended {
            return;
        }
        if let Some(hook) = self.hook.as_mut() {
            hook.all_notes_off();
        }
    }
}

/// The engine's link to MIDI output: `None` until the host sets one.
pub(crate) type SharedMidiOutput = std::sync::Mutex<Option<Arc<dyn MidiOutput>>>;

/// Where the transport is, for a note played live (see [`crate::AudioEngine::transport_position`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportPosition {
    pub pattern: i32,
    pub row: i32,
    /// The position within the song, as the engine reports it.
    pub position: f64,
    pub playing: bool,
}

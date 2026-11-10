//! What MIDI needs from [`AudioEngine`]: the output hook of its renderers, and the two facts a
//! live MIDI note asks for (is note recording armed, where is the transport). A child module of
//! [`crate::engine`], so that these additions stay out of that file. The seam itself is
//! [`crate::midi`].

use super::AudioEngine;
use crate::midi::{MidiOutput, TransportPosition};
use std::sync::Arc;

impl AudioEngine {
    /// Connects MIDI output: the current renderer, and every renderer the engine makes later (a
    /// restart at another sample rate), gets a hook from `output`. `None` disconnects; the hook
    /// in use is dropped on this thread.
    pub fn set_midi_output(&self, output: Option<Arc<dyn MidiOutput>>) {
        *self.midi.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = output;
        self.attach_midi();
    }

    /// Gives the renderer in use a hook from the connected MIDI output (or removes its hook).
    /// Waits for the block being rendered: a device thread only ever `try_lock`s the renderer.
    pub(super) fn attach_midi(&self) {
        let output = self.midi.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let hook = output.map(|output| output.render_hook());
        self.renderer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).midi().set_hook(hook);
    }

    /// Whether note recording is armed (`transport.recordArm`).
    pub fn record_armed(&self) -> bool {
        self.lock().record_armed
    }

    /// Where the transport is now, as the `status` event reports it.
    pub fn transport_position(&self) -> TransportPosition {
        let status = self.lock().core.status.status();
        TransportPosition { pattern: status.pattern, row: status.row, position: status.position, playing: status.playing != 0 }
    }
}

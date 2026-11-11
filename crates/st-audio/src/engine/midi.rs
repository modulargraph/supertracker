//! What MIDI needs from [`AudioEngine`]: the output hook of its renderers, the two facts a
//! live MIDI note asks for (is note recording armed, where is the transport), and where
//! controllers, pressure and pitch bend played live go ([`AudioEngine::expression`]). A child
//! module of [`crate::engine`], so that these additions stay out of that file. The seam itself
//! is [`crate::midi`].

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

    /// A channel message played live for the instrument of `track`: a control change, channel
    /// pressure, poly pressure, pitch bend or a program change. `message[0]` is the status
    /// (its channel does not matter), the data bytes follow.
    ///
    /// - The engine's own voices take what they have a use for: pitch bend (two semitones
    ///   either way) and the modulation wheel (controller 1). Everything else is nothing to them.
    /// - A plug-in instrument on the track gets the message as MIDI at the first frame of its
    ///   next block, in its format's own terms (see `crates/plugin-host`).
    ///
    /// Returns whether something took the message. Notes, channel-mode messages (all notes
    /// off and its relatives, controllers 120 and up) and system messages are refused: notes
    /// have [`AudioEngine::note_on`], silence has [`AudioEngine::panic`]. External MIDI
    /// instruments are the MIDI layer's (`st-midi` sends to their device).
    pub fn expression(&self, track: i32, message: [u8; 3]) -> bool {
        let (kind, data1, data2) = (message[0] & 0xF0, message[1] & 0x7F, message[2] & 0x7F);
        let own = match (kind, data1) {
            (0xB0, 120..) => return false,
            (0xE0, _) => {
                let bend = (i32::from(data1) | i32::from(data2) << 7) - 8192;
                let mut state = self.lock();
                let (control, shared, _) = state.core.control.parts();
                control.pitch_bend(shared, track, bend as f32 / 8192.0 * 2.0);
                true
            }
            (0xB0, 1) => {
                let mut state = self.lock();
                let (control, shared, _) = state.core.control.parts();
                control.mod_wheel(shared, track, f32::from(data2) / 127.0);
                true
            }
            (0xA0 | 0xB0 | 0xC0 | 0xD0, _) => false,
            _ => return false,
        };
        self.plugin_midi(track, [kind, data1, data2]) || own
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

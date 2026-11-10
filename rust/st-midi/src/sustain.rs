//! Sustain-pedal ownership of note releases. Pure control state: no MIDI or audio I/O.

use std::collections::{BTreeSet, HashMap};

/// Where a deferred note-off must go: the engine channel ("track") and pitch the note sounds on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Destination {
    /// Engine channel.
    pub track: usize,
    /// MIDI note number.
    pub note: u8,
}

/// Which pedals are down, per input device and channel, and the note releases each one holds back.
///
/// The destination is part of what a pedal owns: after the player moves to another track and
/// strikes the same key again, the release deferred for the former track is still delivered there.
#[derive(Debug, Clone)]
pub struct SustainState {
    // Presence means the pedal is down, also before any key was released.
    inputs: HashMap<(i32, u8), BTreeSet<Destination>>,
    destination_count: usize,
}

impl Default for SustainState {
    fn default() -> Self {
        Self::new(16)
    }
}

impl SustainState {
    /// State for an engine with `destination_count` channels (tracks and kit sounds).
    pub fn new(destination_count: usize) -> Self {
        SustainState { inputs: HashMap::new(), destination_count }
    }

    /// Whether the pedal of `source` on `channel` (1 to 16) is down.
    pub fn is_down(&self, source: i32, channel: i64) -> bool {
        Self::channel(channel).is_some_and(|channel| self.inputs.contains_key(&(source, channel)))
    }

    fn channel(channel: i64) -> Option<u8> {
        (1..=16).contains(&channel).then_some(channel as u8)
    }

    /// Pedal down or up.
    ///
    /// Admit pedal-down after the current route's input filters, but process pedal-up before them:
    /// notes the pedal owns must still be released after the selection or the input channel
    /// changed. Pedal-up returns the note-offs that were held back, sorted by track then note.
    pub fn set_pedal(&mut self, source: i32, channel: i64, down: bool) -> Vec<Destination> {
        let Some(channel) = Self::channel(channel) else {
            return Vec::new();
        };
        if down {
            self.inputs.entry((source, channel)).or_default();
            return Vec::new();
        }
        self.inputs.remove(&(source, channel)).map(|notes| notes.into_iter().collect()).unwrap_or_default()
    }

    /// A key was released. Returns true when this input's pedal now owns the sounding note-off (the
    /// caller must not release the sound; it still reports the key release itself).
    pub fn defer_note_off(&mut self, source: i32, channel: i64, track: i64, note: i64) -> bool {
        if !(0..self.destination_count as i64).contains(&track) || !(0..128).contains(&note) {
            return false;
        }
        let Some(channel) = Self::channel(channel) else {
            return false;
        };
        match self.inputs.get_mut(&(source, channel)) {
            Some(notes) => {
                notes.insert(Destination { track: track as usize, note: note as u8 });
                true
            }
            None => false,
        }
    }

    /// A note sounds again: only its own deferred note-off is cancelled.
    pub fn retrigger(&mut self, source: i32, channel: i64, track: i64, note: i64) {
        let (Some(channel), Ok(track), Ok(note)) = (Self::channel(channel), usize::try_from(track), u8::try_from(note)) else {
            return;
        };
        if let Some(notes) = self.inputs.get_mut(&(source, channel)) {
            notes.remove(&Destination { track, note });
        }
    }

    /// Tracks moved to other engine channels (`(from, Some(to))`) or were removed (`(from, None)`).
    /// Deferred releases follow their track; all moves apply at once, so swaps work. A removed
    /// track's releases are dropped: its voices are gone already.
    pub fn remap_tracks(&mut self, moves: &[(usize, Option<usize>)]) {
        let moves: HashMap<usize, Option<usize>> = moves.iter().copied().collect();
        let count = self.destination_count;
        for notes in self.inputs.values_mut() {
            *notes = notes
                .iter()
                .filter_map(|held| match moves.get(&held.track) {
                    None => Some(*held),
                    Some(Some(to)) if *to < count => Some(Destination { track: *to, note: held.note }),
                    Some(_) => None,
                })
                .collect();
        }
    }

    /// The input route is torn down: every deferred release, once per destination, sorted; all
    /// pedals are up afterwards.
    pub fn release_all(&mut self) -> Vec<Destination> {
        let all: BTreeSet<Destination> = self.inputs.drain().flat_map(|(_, notes)| notes).collect();
        all.into_iter().collect()
    }

    /// Forgets everything without releasing (a panic already silenced the engine and the devices).
    pub fn clear(&mut self) {
        self.inputs.clear();
    }
}

#[cfg(test)]
mod tests {
    //! Ported from tests/MIDISustainStateTests.swift.
    use super::*;

    fn note(track: usize, note: u8) -> Destination {
        Destination { track, note }
    }

    #[test]
    fn input_source_isolation_and_unrelated_pedal_up() {
        let mut state = SustainState::default();
        state.set_pedal(10, 1, true);
        assert!(state.defer_note_off(10, 1, 0, 60), "A must sustain");
        assert!(!state.defer_note_off(20, 1, 1, 64), "A must not sustain source B");
        assert!(state.set_pedal(20, 1, false).is_empty(), "B pedal-up must not release A");
        assert!(state.is_down(10, 1), "B must not disarm A");
        assert_eq!(state.set_pedal(10, 1, false), [note(0, 60)], "A exact release");
    }

    #[test]
    fn channel_isolation_on_the_same_input_source() {
        let mut state = SustainState::default();
        state.set_pedal(10, 1, true);
        assert!(!state.defer_note_off(10, 2, 0, 60), "channel 1 must not sustain channel 2");
        state.set_pedal(10, 2, true);
        state.defer_note_off(10, 1, 2, 60);
        state.defer_note_off(10, 2, 3, 60);
        assert_eq!(state.set_pedal(10, 2, false), [note(3, 60)], "channel 2 exact release");
        assert_eq!(state.set_pedal(10, 1, false), [note(2, 60)], "channel 1 survived");
    }

    #[test]
    fn retrigger_cancels_only_the_exact_input_track_and_note() {
        let mut state = SustainState::default();
        state.set_pedal(10, 1, true);
        state.defer_note_off(10, 1, 1, 60);
        state.defer_note_off(10, 1, 1, 62);
        state.defer_note_off(10, 1, 2, 60);
        state.retrigger(10, 1, 2, 60);
        state.retrigger(20, 1, 1, 60);
        state.retrigger(10, 2, 1, 62);
        assert_eq!(state.set_pedal(10, 1, false), [note(1, 60), note(1, 62)]);
    }

    #[test]
    fn a_captured_destination_survives_selection_and_filter_changes() {
        let mut state = SustainState::default();
        state.set_pedal(10, 1, true);
        // The note-off owns its earlier destination even though the selected target now accepts
        // channel 2 only: the host processes the owned release before the new filter.
        let original = note(4, 67);
        assert!(state.defer_note_off(10, 1, original.track as i64, i64::from(original.note)));
        assert_eq!(state.set_pedal(10, 1, false), [original]);
        assert!(!state.is_down(10, 1), "old route pedal reset");
    }

    #[test]
    fn repeated_controller_messages_and_retrigger_release_cycle() {
        let mut state = SustainState::default();
        state.set_pedal(10, 1, true);
        state.defer_note_off(10, 1, 0, 60);
        state.set_pedal(10, 1, true);
        state.defer_note_off(10, 1, 0, 60);
        assert_eq!(state.set_pedal(10, 1, false), [note(0, 60)], "repeated messages neither lose nor duplicate the off");
        assert!(state.set_pedal(10, 1, false).is_empty(), "repeated pedal-up is inert");
        state.set_pedal(10, 1, true);
        state.defer_note_off(10, 1, 0, 60);
        state.retrigger(10, 1, 0, 60);
        assert!(state.is_down(10, 1), "retrigger retains the pedal");
        assert!(state.defer_note_off(10, 1, 0, 60), "a retriggered key-up defers again");
        assert_eq!(state.release_all(), [note(0, 60)]);
    }

    #[test]
    fn route_teardown_panic_and_idempotent_clearing() {
        let mut state = SustainState::default();
        for (source, channel) in [(10, 1), (20, 2)] {
            state.set_pedal(source, channel, true);
            state.defer_note_off(source, channel, 2, 70);
            state.defer_note_off(source, channel, 0, 60);
        }
        assert_eq!(state.release_all(), [note(0, 60), note(2, 70)], "teardown returns each destination once");
        assert!(state.release_all().is_empty() && !state.is_down(10, 1), "teardown clears pedals");
        state.set_pedal(10, 1, true);
        state.defer_note_off(10, 1, 1, 64);
        state.clear();
        state.clear();
        assert!(state.release_all().is_empty() && !state.is_down(10, 1), "a panic never releases old state into a new session");
    }

    #[test]
    fn input_validation_and_exact_boundary_values() {
        let mut state = SustainState::default();
        for channel in [-1, 0, 17, i64::MAX] {
            state.set_pedal(10, channel, true);
            assert!(!state.is_down(10, channel), "an invalid channel must not arm");
            assert!(!state.defer_note_off(10, channel, 0, 60), "an invalid channel must not defer");
        }
        state.set_pedal(i32::MIN, 16, true);
        for (track, pitch) in [(-1, 60), (16, 60), (0, -1), (0, 128)] {
            assert!(!state.defer_note_off(i32::MIN, 16, track, pitch), "invalid destination");
        }
        assert!(state.defer_note_off(i32::MIN, 16, 15, 127), "a negative device id and the last destination are valid");
        assert_eq!(state.set_pedal(i32::MIN, 16, false), [note(15, 127)]);
    }

    #[test]
    fn configured_kit_destinations_sustain_through_the_last_channel() {
        let mut state = SustainState::new(80);
        state.set_pedal(10, 1, true);
        for track in [0, 15, 16, 31, 64, 79] {
            assert!(state.defer_note_off(10, 1, track, 60), "ordinary and kit destinations must sustain");
        }
        assert!(!state.defer_note_off(10, 1, 80, 60), "no channel beyond the engine's capacity");
        state.retrigger(10, 1, 79, 60);
        assert_eq!(state.set_pedal(10, 1, false), [0, 15, 16, 31, 64].map(|track| note(track, 60)));
        state.set_pedal(20, 16, true);
        assert!(state.defer_note_off(20, 16, 79, 127));
        assert_eq!(state.release_all(), [note(79, 127)]);
    }

    #[test]
    fn deferred_releases_follow_moved_kit_sounds() {
        let mut state = SustainState::new(80);
        state.set_pedal(10, 1, true);
        for (track, pitch) in [(16, 36), (17, 38), (18, 42), (3, 60)] {
            state.defer_note_off(10, 1, track, pitch);
        }
        state.remap_tracks(&[(16, Some(17)), (17, Some(16)), (18, None)]);
        assert_eq!(state.set_pedal(10, 1, false), [note(3, 60), note(16, 38), note(17, 36)], "pedal-up releases the moved sounds' new channels only");
    }
}

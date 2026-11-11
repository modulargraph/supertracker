//! Where incoming MIDI goes: which track and instrument a device plays, the chord map of the
//! keyboard performance settings, and transport control. Pedals are routed like notes and handed
//! on as they are: what a pedal holds is decided where the instruments are known
//! ([`crate::sustain`]).
//!
//! Pure state: [`InputRouter::handle`] turns one message into a list of [`InputAction`]s, which
//! [`crate::MidiService`] carries out (on the engine through [`crate::InputTarget`], on devices
//! through the host). Nothing here touches a device or the audio engine.

use std::collections::HashMap;

use serde_json::Value;

use crate::message::Message;
use crate::sustain::{PEDALS, SUSTAIN};

/// The MIDI side of one instrument of the project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstrumentRoute {
    /// An external MIDI instrument (type `midi`): it sounds on a device, not in the engine.
    pub external: bool,
    /// The input device it listens to, as stored (`midiInput`): a device id as text, or `"all"` or
    /// `"0"` for every enabled input. `None` when the project does not say.
    pub input: Option<String>,
    /// The input channel it listens to (1 to 16), or 0 for all.
    pub input_channel: u8,
    /// The output device of an external instrument (`midiOutput`); `None` is the default output.
    pub output: Option<i32>,
    /// The output channel, 0 to 15 (`midiChannel` - 1).
    pub channel: u8,
}

/// The MIDI side of one track of the project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackRoute {
    /// The track's instrument.
    pub instrument: usize,
    /// The track is frozen: it cannot be played live.
    pub frozen: bool,
}

/// What routing needs from the project. `st-host` rebuilds it whenever the project is published.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputRouting {
    /// Instruments, as the engine numbers them.
    pub instruments: Vec<InstrumentRoute>,
    /// Tracks (engine channels), in order.
    pub tracks: Vec<TrackRoute>,
}

fn integer(value: Option<&Value>, fallback: i64) -> i64 {
    match value {
        Some(Value::Number(number)) => number.as_i64().or_else(|| number.as_f64().map(|float| float as i64)).unwrap_or(fallback),
        Some(Value::String(text)) => text.trim().parse().unwrap_or(fallback),
        Some(Value::Bool(flag)) => i64::from(*flag),
        _ => fallback,
    }
}

impl InputRouting {
    /// Reads the routing out of the project document the interface publishes: `instruments[]`
    /// (`type`, `midiInput`, `midiInputChannel`, `midiOutput`, `midiChannel`) and `tracks[]`
    /// (`instrument`, `frozen`).
    pub fn from_project(project: &Value) -> Self {
        let list = |key: &str| project.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let instruments = list("instruments")
            .iter()
            .take(crate::output::ROUTE_COUNT)
            .map(|item| InstrumentRoute {
                external: item.get("type").and_then(Value::as_str) == Some("midi"),
                input: match item.get("midiInput") {
                    Some(Value::String(text)) => Some(text.clone()),
                    Some(Value::Number(number)) => Some(number.as_i64().map_or_else(|| number.to_string(), |id| id.to_string())),
                    _ => None,
                },
                input_channel: integer(item.get("midiInputChannel"), 0).clamp(0, 16) as u8,
                output: i32::try_from(integer(item.get("midiOutput"), 0)).ok().filter(|id| *id != 0),
                channel: (integer(item.get("midiChannel"), 1) - 1).clamp(0, 15) as u8,
            })
            .collect();
        let tracks = list("tracks")
            .iter()
            .map(|item| TrackRoute {
                instrument: usize::try_from(integer(item.get("instrument"), 0)).unwrap_or(0),
                frozen: item.get("frozen").is_some_and(|record| !record.is_null()),
            })
            .collect();
        InputRouting { instruments, tracks }
    }

    /// The highest engine channel a note may go to (the engine keeps at least 16 channels).
    pub fn last_track(&self) -> usize {
        self.tracks.len().max(16) - 1
    }

    /// The highest instrument index.
    pub fn last_instrument(&self) -> usize {
        self.instruments.len().clamp(1, crate::output::ROUTE_COUNT) - 1
    }
}

/// A note with the place it plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutedNote {
    /// MIDI note number.
    pub note: u8,
    /// Velocity (of the press, or of the release).
    pub velocity: u8,
    /// MIDI channel it arrived on, 1 to 16.
    pub channel: u8,
    /// Id of the input device.
    pub source: i32,
    /// Engine channel it plays on.
    pub track: usize,
    /// Instrument it plays.
    pub instrument: usize,
}

/// A pedal message with the place it is played to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutedPedal {
    /// 64 (sustain), 66 (sostenuto) or 67 (soft).
    pub controller: u8,
    /// 0 to 127.
    pub value: u8,
    /// MIDI channel it arrived on, 1 to 16.
    pub channel: u8,
    /// Id of the input device.
    pub source: i32,
    /// The engine channel and instrument a note of this device and channel would play now;
    /// `None` when its input is refused there. A pedal is handed on all the same: what it holds
    /// must be let go when it comes up, also after the selection or the input filter changed.
    pub destination: Option<(usize, usize)>,
}

/// One thing to do for an incoming message.
#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    /// Start a note.
    NoteOn(RoutedNote),
    /// A key was released: the note, at the place its note-on went to.
    NoteOff(RoutedNote),
    /// A key is released in its device's name because the input route changed. Not reported
    /// to the interface.
    Release(RoutedNote),
    /// A sustain, sostenuto or soft pedal moved.
    Pedal(RoutedPedal),
    /// The input route changed: no pedal-up will arrive through it, so every pedal of a device
    /// counts as lifted.
    LiftPedals,
    /// Pass a controller, program change, channel or poly pressure or pitch-bend message on to
    /// the instrument: to its device when it is external, to the engine otherwise (its own
    /// voices and a plug-in instrument on the track). `message.status` carries the kind only
    /// (channel 0).
    Forward {
        /// Engine channel.
        track: usize,
        /// Instrument.
        instrument: usize,
        /// The message, without a channel.
        message: Message,
    },
    /// Tell the interface about a control change (MIDI learn, mapped controls).
    Control {
        /// The message as received.
        message: Message,
        /// Id of the input device.
        source: i32,
    },
    /// All sound off or all notes off arrived: silence the engine and every external instrument.
    Panic,
    /// Start or Continue arrived: play.
    Start,
    /// Stop arrived: stop, and silence external instruments.
    Stop,
    /// A note was played on a frozen track and refused.
    Frozen {
        /// The track.
        track: usize,
    },
}

/// Routing state for incoming MIDI. One instance, used from one thread at a time.
#[derive(Debug, Default)]
pub struct InputRouter {
    routing: InputRouting,
    track: usize,
    instrument: usize,
    input_channel: u8,
    blocked: bool,
    /// (source, channel 1–16, note) → where its note-on went.
    note_destinations: HashMap<(i32, u8, u8), (usize, usize)>,
    /// (source, channel 0–15, key) → the notes the performance map played for it.
    expanded: HashMap<(i32, u8, u8), Vec<u8>>,
    performance: Option<Vec<Vec<u8>>>,
}

impl InputRouter {
    /// A router without a project: input plays track 0 with instrument 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// The project's routing.
    pub fn routing(&self) -> &InputRouting {
        &self.routing
    }

    /// Replaces the project's routing.
    pub fn set_routing(&mut self, routing: InputRouting) {
        self.routing = routing;
    }

    /// The track and instrument that input plays when no instrument claims the device.
    pub fn destination(&self) -> (usize, usize) {
        (self.track, self.instrument)
    }

    /// Selects the track and instrument that input plays, within the project's range.
    pub fn set_destination(&mut self, track: i64, instrument: i64) {
        self.track = track.clamp(0, self.routing.last_track() as i64) as usize;
        self.instrument = instrument.clamp(0, self.routing.last_instrument() as i64) as usize;
    }

    /// The input channel filter (0: all).
    pub fn input_channel(&self) -> u8 {
        self.input_channel
    }

    /// Sets the input channel filter (0: all).
    pub fn set_input_channel(&mut self, channel: u8) {
        self.input_channel = channel.min(16);
    }

    /// While blocked (an export runs, the project cannot be played) input is dropped.
    pub fn set_blocked(&mut self, blocked: bool) {
        self.blocked = blocked;
    }

    /// Sets the chord map of the keyboard performance settings: for each of the 128 keys, the up
    /// to eight notes it plays. Anything else, and the map where every key plays itself, turns it
    /// off. Returns whether a map is active.
    pub fn set_performance_map(&mut self, value: &Value) -> bool {
        let rows = value.as_array().filter(|rows| rows.len() == 128 && rows.iter().all(Value::is_array));
        self.performance = rows.and_then(|rows| {
            let map: Vec<Vec<u8>> = rows
                .iter()
                .map(|row| {
                    row.as_array().into_iter().flatten().filter_map(|note| note.as_f64()).filter(|note| (0.0..=127.0).contains(note) && note.fract() == 0.0).map(|note| note as u8).take(8).collect()
                })
                .collect();
            let identity = map.iter().enumerate().all(|(key, notes)| notes.len() == 1 && usize::from(notes[0]) == key);
            (!identity).then_some(map)
        });
        self.performance.is_some()
    }

    /// Handles one input message from `source`. `clock_active` tells whether the application sends
    /// MIDI clock itself: transport messages are ignored then, because a device echoing our own
    /// Start would restart the transport.
    pub fn handle(&mut self, message: Message, source: i32, clock_active: bool) -> Vec<InputAction> {
        let mut actions = Vec::new();
        let kind = message.status & 0xF0;
        if message.status >= 0xF0 || !(kind == 0x90 || kind == 0x80) || message.data1 >= 128 {
            self.direct(message, source, clock_active, &mut actions);
            return actions;
        }
        let key = (source, message.status & 0x0F, message.data1);
        if kind == 0x90 && message.data2 > 0 {
            // A key struck again first releases what it was playing.
            for note in self.expanded.remove(&key).unwrap_or_default() {
                self.direct(Message::new(0x80 | (message.status & 0x0F), note, 0), source, clock_active, &mut actions);
            }
            match self.performance.as_ref().map(|map| map[usize::from(message.data1)].clone()) {
                None => self.direct(message, source, clock_active, &mut actions),
                Some(notes) => {
                    for &note in &notes {
                        self.direct(Message::new(message.status, note, message.data2), source, clock_active, &mut actions);
                    }
                    self.expanded.insert(key, notes);
                }
            }
        } else {
            // A later change of the map does not change what a held key owns; a key that owns
            // nothing releases its own pitch.
            for note in self.expanded.remove(&key).unwrap_or_else(|| vec![message.data1]) {
                self.direct(Message::new(message.status, note, message.data2), source, clock_active, &mut actions);
            }
        }
        actions
    }

    fn direct(&mut self, message: Message, source: i32, clock_active: bool, actions: &mut Vec<InputAction>) {
        if self.blocked {
            return;
        }
        let (status, data1, data2) = (message.status, message.data1, message.data2);
        if status >= 0xF0 {
            // Of the system messages only Start, Continue and Stop mean anything here.
            if !clock_active {
                match status {
                    0xFA | 0xFB => actions.push(InputAction::Start),
                    0xFC => actions.push(InputAction::Stop),
                    _ => {}
                }
            }
            return;
        }
        let channel = (status & 0x0F) + 1;
        let kind = status & 0xF0;
        // Releases belong to the input that started the note, also after the selection changed to
        // an instrument that would refuse new notes from it.
        if kind == 0x80 || (kind == 0x90 && data2 == 0) {
            // A release this input never started here owns nothing: dropping it keeps it from
            // cutting the same pitch held on the selected track by another device.
            let Some((track, instrument)) = self.note_destinations.remove(&(source, channel, data1)) else { return };
            actions.push(InputAction::NoteOff(RoutedNote { note: data1, velocity: data2, channel, source, track, instrument }));
            return;
        }
        let destination = self.destination_of(source, channel);
        if kind == 0xB0 && PEDALS.contains(&data1) {
            // A pedal is handed on whether or not its input is accepted here: what it holds must
            // be let go when it comes up.
            actions.push(InputAction::Pedal(RoutedPedal { controller: data1, value: data2, channel, source, destination }));
            // The sustain pedal is no controller to learn or map; the other two are reported
            // like any control change.
            if data1 != SUSTAIN && destination.is_some() {
                actions.push(InputAction::Control { message, source });
            }
            return;
        }
        let Some((track, instrument)) = destination else { return };
        match kind {
            0x90 => {
                if self.routing.tracks.get(track).is_some_and(|item| item.frozen) {
                    actions.push(InputAction::Frozen { track });
                    return;
                }
                self.note_destinations.insert((source, channel, data1), (track, instrument));
                actions.push(InputAction::NoteOn(RoutedNote { note: data1, velocity: data2, channel, source, track, instrument }));
            }
            0xB0 if data1 == 120 || data1 == 123 => actions.push(InputAction::Panic),
            0xA0 | 0xB0 | 0xC0 | 0xD0 | 0xE0 => {
                actions.push(InputAction::Forward { track, instrument: instrument.min(self.routing.last_instrument()), message: Message::new(kind, data1, data2) });
                // The interface uses control changes only. Pitch bend and aftertouch stream at
                // hundreds of messages a second and are not reported.
                if kind == 0xB0 {
                    actions.push(InputAction::Control { message, source });
                }
            }
            _ => {}
        }
    }

    /// The engine channel and instrument new input of `source` on `channel` (1 to 16) plays: the
    /// selected ones, or those of an external instrument bound to the device. `None` when the
    /// input channel filter or the selected instrument's input filter refuses it.
    fn destination_of(&self, source: i32, channel: u8) -> Option<(usize, usize)> {
        if self.input_channel != 0 && self.input_channel != channel {
            return None;
        }
        let claimed = self.routing.instruments.iter().position(|item| {
            item.external && item.input.as_deref() == Some(source.to_string().as_str()) && (item.input_channel == 0 || item.input_channel == channel)
        });
        if let Some(index) = claimed {
            // An external instrument bound to this device takes its input wherever the cursor is.
            return Some((self.routing.tracks.iter().position(|item| item.instrument == index).unwrap_or(self.track), index));
        }
        if let Some(item) = self.routing.instruments.get(self.instrument) {
            if let Some(input) = &item.input {
                if !(input == "all" || input == "0" || *input == source.to_string()) {
                    return None;
                }
            }
            if item.input_channel != 0 && item.input_channel != channel {
                return None;
            }
        }
        Some((self.track, self.instrument))
    }

    /// The input route changed: every key input still holds is released in its device's name
    /// and its pedals count as lifted; the router forgets the keys.
    pub fn release_all(&mut self) -> Vec<InputAction> {
        let mut held: Vec<RoutedNote> = self
            .note_destinations
            .iter()
            .map(|((source, channel, note), (track, instrument))| RoutedNote { note: *note, velocity: 0, channel: *channel, source: *source, track: *track, instrument: *instrument })
            .collect();
        held.sort_by_key(|note| (note.track, note.note, note.source, note.channel));
        let mut actions: Vec<InputAction> = held.into_iter().map(InputAction::Release).collect();
        actions.push(InputAction::LiftPedals);
        self.note_destinations.clear();
        self.expanded.clear();
        actions
    }

    /// A panic silenced everything already: forget held keys without releasing them.
    pub fn clear(&mut self) {
        self.note_destinations.clear();
        self.expanded.clear();
    }

    /// Tracks moved to other engine channels (`Some(to)`) or were removed (`None`): held keys
    /// follow their track, so later releases reach the right channel.
    pub fn remap_tracks(&mut self, moves: &[(usize, Option<usize>)]) {
        let moves: HashMap<usize, Option<usize>> = moves.iter().copied().collect();
        self.note_destinations.retain(|_, destination| match moves.get(&destination.0) {
            None => true,
            Some(Some(to)) => {
                destination.0 = *to;
                true
            }
            Some(None) => false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn router() -> InputRouter {
        let mut router = InputRouter::new();
        router.set_routing(InputRouting::from_project(&json!({
            "instruments": [
                {"type": "sample", "midiInput": 0, "midiInputChannel": 0, "midiOutput": 0, "midiChannel": 1},
                {"type": "midi", "midiInput": 77, "midiInputChannel": 2, "midiOutput": 500, "midiChannel": 10},
                {"type": "sample", "midiInput": "9", "midiInputChannel": 3},
                {"type": "midi", "midiOutput": "600", "midiChannel": 99},
            ],
            "tracks": [{"instrument": 0}, {"instrument": 2}, {"instrument": 1}, {"instrument": 0, "frozen": {"path": "x.wav"}}, {"instrument": 0, "frozen": null}],
        })));
        router
    }

    fn note(note: u8, velocity: u8, channel: u8, source: i32, track: usize, instrument: usize) -> RoutedNote {
        RoutedNote { note, velocity, channel, source, track, instrument }
    }

    #[test]
    fn the_project_is_read_leniently() {
        let routing = router().routing().clone();
        assert_eq!(routing.instruments[0], InstrumentRoute { external: false, input: Some("0".into()), input_channel: 0, output: None, channel: 0 });
        assert_eq!(routing.instruments[1], InstrumentRoute { external: true, input: Some("77".into()), input_channel: 2, output: Some(500), channel: 9 });
        assert_eq!(routing.instruments[2].input.as_deref(), Some("9"));
        assert_eq!(routing.instruments[3], InstrumentRoute { external: true, input: None, input_channel: 0, output: Some(600), channel: 15 });
        assert_eq!(routing.tracks.iter().map(|track| track.frozen).collect::<Vec<_>>(), [false, false, false, true, false]);
        assert_eq!((routing.last_track(), routing.last_instrument()), (15, 3));
        assert_eq!(InputRouting::from_project(&json!({})), InputRouting::default());
        assert_eq!(InputRouting::default().last_instrument(), 0);
    }

    #[test]
    fn notes_play_the_selected_destination_and_release_where_they_started() {
        let mut router = router();
        assert_eq!(router.handle(Message::new(0x90, 60, 100), 5, false), [InputAction::NoteOn(note(60, 100, 1, 5, 0, 0))]);
        // The selection moves while the key is down: the release still goes to track 0.
        router.set_destination(1, 2);
        assert_eq!(router.destination(), (1, 2));
        assert_eq!(router.handle(Message::new(0x80, 60, 40), 5, false), [InputAction::NoteOff(note(60, 40, 1, 5, 0, 0))]);
        assert!(router.handle(Message::new(0x80, 60, 0), 5, false).is_empty(), "a release that owns nothing is dropped");
        assert!(router.handle(Message::new(0x90, 61, 0), 5, false).is_empty(), "also as a note-on with velocity zero");
        // Instrument 2 listens to device 9 on channel 3 only.
        assert!(router.handle(Message::new(0x92, 60, 100), 5, false).is_empty(), "another device is refused");
        assert!(router.handle(Message::new(0x90, 60, 100), 9, false).is_empty(), "another channel is refused");
        assert_eq!(router.handle(Message::new(0x92, 60, 100), 9, false), [InputAction::NoteOn(note(60, 100, 3, 9, 1, 2))]);
        router.set_destination(99, 99);
        assert_eq!(router.destination(), (15, 3), "the destination stays inside the project");
    }

    #[test]
    fn an_external_instrument_claims_its_device_and_channel() {
        let mut router = router();
        // Device 77 on channel 2 belongs to instrument 1, which track 2 plays.
        assert_eq!(router.handle(Message::new(0x91, 64, 90), 77, false), [InputAction::NoteOn(note(64, 90, 2, 77, 2, 1))]);
        assert_eq!(router.handle(Message::new(0x90, 64, 90), 77, false), [InputAction::NoteOn(note(64, 90, 1, 77, 0, 0))], "other channels of the device play the selection");
        assert_eq!(
            router.handle(Message::new(0xB1, 7, 100), 77, false),
            [InputAction::Forward { track: 2, instrument: 1, message: Message::new(0xB0, 7, 100) }, InputAction::Control { message: Message::new(0xB1, 7, 100), source: 77 }]
        );
    }

    #[test]
    fn the_input_channel_filter_applies_to_new_input_only() {
        let mut router = router();
        let pedal = |value: u8, channel: u8, destination: Option<(usize, usize)>| InputAction::Pedal(RoutedPedal { controller: 64, value, channel, source: 5, destination });
        router.handle(Message::new(0x90, 60, 100), 5, false);
        assert_eq!(router.handle(Message::new(0xB0, 64, 127), 5, false), [pedal(127, 1, Some((0, 0)))]);
        router.set_input_channel(4);
        assert_eq!(router.input_channel(), 4);
        assert!(router.handle(Message::new(0x90, 62, 100), 5, false).is_empty());
        assert_eq!(router.handle(Message::new(0x93, 62, 100), 5, false), [InputAction::NoteOn(note(62, 100, 4, 5, 0, 0))]);
        // What channel 1 owns is still released: its key, and its pedal (which is handed on
        // without a place to play).
        assert_eq!(router.handle(Message::new(0x80, 60, 0), 5, false), [InputAction::NoteOff(note(60, 0, 1, 5, 0, 0))]);
        assert_eq!(router.handle(Message::new(0xB0, 64, 0), 5, false), [pedal(0, 1, None)]);
        assert_eq!(router.handle(Message::new(0xB0, 64, 90), 5, false), [pedal(90, 1, None)], "a refused pedal is still handed on: it may hold something");
    }

    #[test]
    fn pedals_are_routed_like_notes_and_handed_on_with_their_values() {
        let mut router = router();
        let pedal = |controller: u8, value: u8, channel: u8, source: i32, destination: Option<(usize, usize)>| InputAction::Pedal(RoutedPedal { controller, value, channel, source, destination });
        // The sustain pedal, half way and all the way; never reported as a control change.
        assert_eq!(router.handle(Message::new(0xB0, 64, 50), 5, false), [pedal(64, 50, 1, 5, Some((0, 0)))]);
        assert_eq!(router.handle(Message::new(0xB0, 64, 127), 5, false), [pedal(64, 127, 1, 5, Some((0, 0)))]);
        // Sostenuto and soft are pedals too; the interface can learn them like other controllers.
        for controller in [66, 67] {
            let message = Message::new(0xB0, controller, 127);
            assert_eq!(router.handle(message, 5, false), [pedal(controller, 127, 1, 5, Some((0, 0))), InputAction::Control { message, source: 5 }]);
        }
        // The keyboard is pointed at another track: the pedal goes where the notes go.
        router.set_destination(4, 3);
        assert_eq!(router.handle(Message::new(0xB0, 64, 0), 5, false), [pedal(64, 0, 1, 5, Some((4, 3)))]);
        // Device 77 on channel 2 belongs to the external instrument 1 on track 2.
        assert_eq!(router.handle(Message::new(0xB1, 64, 127), 77, false), [pedal(64, 127, 2, 77, Some((2, 1)))]);
        // Instrument 2 listens to device 9 on channel 3 only: another device's pedal has no place.
        router.set_destination(1, 2);
        assert_eq!(router.handle(Message::new(0xB0, 66, 127), 5, false), [pedal(66, 127, 1, 5, None)], "handed on, not reported");
        assert_eq!(router.handle(Message::new(0xB2, 64, 127), 9, false), [pedal(64, 127, 3, 9, Some((1, 2)))]);
        router.set_blocked(true);
        assert!(router.handle(Message::new(0xB2, 64, 0), 9, false).is_empty());
    }

    #[test]
    fn controllers_pitch_bend_and_panic() {
        let mut router = router();
        assert_eq!(router.handle(Message::new(0xE0, 0, 96), 5, false), [InputAction::Forward { track: 0, instrument: 0, message: Message::new(0xE0, 0, 96) }]);
        assert_eq!(router.handle(Message::new(0xD0, 55, 0), 5, false), [InputAction::Forward { track: 0, instrument: 0, message: Message::new(0xD0, 55, 0) }]);
        assert_eq!(router.handle(Message::new(0xC0, 3, 0), 5, false).len(), 1);
        assert_eq!(router.handle(Message::new(0xA0, 60, 9), 5, false).len(), 1);
        assert_eq!(router.handle(Message::new(0xB0, 123, 0), 5, false), [InputAction::Panic]);
        assert_eq!(router.handle(Message::new(0xB0, 120, 0), 5, false), [InputAction::Panic]);
    }

    #[test]
    fn transport_messages_are_followed_unless_we_send_clock_ourselves() {
        let mut router = router();
        router.set_input_channel(3);
        assert_eq!(router.handle(Message::new(0xFA, 0, 0), 5, false), [InputAction::Start]);
        assert_eq!(router.handle(Message::new(0xFB, 0, 0), 5, false), [InputAction::Start]);
        assert_eq!(router.handle(Message::new(0xFC, 0, 0), 5, false), [InputAction::Stop]);
        for status in [0xF8, 0xF9, 0xFE, 0xFF, 0xF2, 0xF6] {
            assert!(router.handle(Message::new(status, 0, 0), 5, false).is_empty());
        }
        for status in [0xFA, 0xFB, 0xFC] {
            assert!(router.handle(Message::new(status, 0, 0), 5, true).is_empty(), "an echoed Start must not restart our own transport");
        }
    }

    #[test]
    fn frozen_tracks_refuse_notes_and_blocked_input_is_dropped() {
        let mut router = router();
        router.set_destination(3, 0);
        assert_eq!(router.handle(Message::new(0x90, 60, 100), 5, false), [InputAction::Frozen { track: 3 }]);
        assert!(router.handle(Message::new(0x80, 60, 0), 5, false).is_empty());
        router.set_destination(4, 0);
        assert_eq!(router.handle(Message::new(0x90, 60, 100), 5, false).len(), 1, "a null freeze record is no freeze");
        router.set_blocked(true);
        assert!(router.handle(Message::new(0x90, 61, 100), 5, false).is_empty());
        assert!(router.handle(Message::new(0xFA, 0, 0), 5, false).is_empty());
    }

    #[test]
    fn the_performance_map_expands_keys_and_remembers_what_they_own() {
        let mut router = router();
        let mut rows: Vec<Value> = (0..128).map(|key| json!([key])).collect();
        assert!(!router.set_performance_map(&json!(rows)), "the identity map is off");
        rows[60] = json!([60, 64, 67, 300, "x", 1.5]);
        rows[61] = json!([]);
        assert!(router.set_performance_map(&json!(rows)));
        let on = router.handle(Message::new(0x90, 60, 100), 5, false);
        assert_eq!(on, [60, 64, 67].map(|pitch| InputAction::NoteOn(note(pitch, 100, 1, 5, 0, 0))));
        assert!(router.handle(Message::new(0x90, 61, 100), 5, false).is_empty(), "a key mapped to nothing is silent");
        // The map goes away while the chord is held: the key still releases its three notes.
        assert!(!router.set_performance_map(&Value::Null));
        let off = router.handle(Message::new(0x80, 60, 0), 5, false);
        assert_eq!(off, [60, 64, 67].map(|pitch| InputAction::NoteOff(note(pitch, 0, 1, 5, 0, 0))));
        // Without a map, striking a held key again is an ordinary retrigger.
        router.handle(Message::new(0x90, 62, 100), 5, false);
        assert_eq!(router.handle(Message::new(0x90, 62, 90), 5, false), [InputAction::NoteOn(note(62, 90, 1, 5, 0, 0))]);
        assert!(!router.set_performance_map(&json!([[1]])), "a map of the wrong size is refused");
    }

    #[test]
    fn a_remapped_key_struck_again_releases_its_chord_first() {
        let mut router = router();
        let mut rows: Vec<Value> = (0..128).map(|key| json!([key])).collect();
        rows[60] = json!([48, 55]);
        router.set_performance_map(&json!(rows));
        router.handle(Message::new(0x90, 60, 100), 5, false);
        let again = router.handle(Message::new(0x90, 60, 80), 5, false);
        assert_eq!(
            again,
            [
                InputAction::NoteOff(note(48, 0, 1, 5, 0, 0)),
                InputAction::NoteOff(note(55, 0, 1, 5, 0, 0)),
                InputAction::NoteOn(note(48, 80, 1, 5, 0, 0)),
                InputAction::NoteOn(note(55, 80, 1, 5, 0, 0)),
            ]
        );
    }

    #[test]
    fn route_changes_release_everything_and_moved_tracks_keep_their_notes() {
        let mut router = router();
        router.handle(Message::new(0x90, 60, 100), 5, false);
        router.handle(Message::new(0x90, 62, 100), 6, false);
        router.handle(Message::new(0xB0, 64, 127), 5, false);
        router.handle(Message::new(0x91, 70, 100), 5, false);
        router.handle(Message::new(0x90, 72, 100), 5, false);
        router.handle(Message::new(0x80, 72, 0), 5, false);
        // Track 0 becomes track 7.
        router.remap_tracks(&[(0, Some(7))]);
        assert_eq!(router.handle(Message::new(0x80, 62, 0), 6, false), [InputAction::NoteOff(note(62, 0, 1, 6, 7, 0))]);
        // The keys still down are released in their devices' names; the pedals count as lifted.
        let released = router.release_all();
        assert_eq!(released, [InputAction::Release(note(60, 0, 1, 5, 7, 0)), InputAction::Release(note(70, 0, 2, 5, 7, 0)), InputAction::LiftPedals]);
        assert_eq!(router.release_all(), [InputAction::LiftPedals]);
        assert!(router.handle(Message::new(0x80, 60, 0), 5, false).is_empty(), "released notes are forgotten");
        router.handle(Message::new(0x90, 60, 100), 5, false);
        router.remap_tracks(&[(0, None)]);
        assert!(router.handle(Message::new(0x80, 60, 0), 5, false).is_empty(), "a removed track's notes are gone with it");
        router.handle(Message::new(0x90, 60, 100), 5, false);
        router.clear();
        assert_eq!(router.release_all(), [InputAction::LiftPedals], "a panic forgets without releasing");
    }
}

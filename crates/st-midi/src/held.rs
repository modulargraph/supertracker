//! What each input device is holding down. A device that is unplugged can never send the releases
//! it owes, so the host sends them in its name. Pure: no devices.

use std::collections::{BTreeSet, HashMap};

use crate::message::Message;
use crate::sustain::PEDALS;

/// Keys and pedals (sustain, sostenuto, soft) held per input device, from the messages delivered
/// for it.
#[derive(Debug, Default)]
pub struct HeldInputs {
    /// `channel << 7 | note`, channel 0 to 15.
    keys: HashMap<i32, BTreeSet<u16>>,
    /// `(channel 0 to 15, controller)` of the pedals that are not all the way up.
    pedals: HashMap<i32, BTreeSet<(u8, u8)>>,
}

impl HeldInputs {
    /// Nothing held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a message delivered from `source`.
    pub fn track(&mut self, source: i32, message: &Message) {
        if message.status < 0x80 || message.status >= 0xF0 || crate::message::data_length(message.status) != 2 {
            return;
        }
        let (kind, channel) = (message.status & 0xF0, message.status & 0x0F);
        let key = u16::from(channel) << 7 | u16::from(message.data1 & 0x7F);
        if kind == 0x90 && message.data2 > 0 {
            self.keys.entry(source).or_default().insert(key);
        } else if kind == 0x80 || kind == 0x90 {
            if let Some(keys) = self.keys.get_mut(&source) {
                keys.remove(&key);
            }
        } else if kind == 0xB0 && PEDALS.contains(&message.data1) {
            // Any value above zero reached an instrument (half-pedalling): it is owed a pedal-up.
            if message.data2 > 0 {
                self.pedals.entry(source).or_default().insert((channel, message.data1));
            } else if let Some(pedals) = self.pedals.get_mut(&source) {
                pedals.remove(&(channel, message.data1));
            }
        } else if kind == 0xB0 && (message.data1 == 120 || message.data1 == 123) {
            // All sound off / all notes off: the device released its keys on this channel itself.
            if let Some(keys) = self.keys.get_mut(&source) {
                keys.retain(|key| key >> 7 != u16::from(channel));
            }
        }
    }

    /// Forgets `source` and returns the releases it still owes: a note-off for every held key, then
    /// pedal-up for every held pedal, each in ascending order.
    pub fn take_releases(&mut self, source: i32) -> Vec<Message> {
        let keys = self.keys.remove(&source).unwrap_or_default();
        let pedals = self.pedals.remove(&source).unwrap_or_default();
        keys.into_iter()
            .map(|key| Message::new(0x80 | (key >> 7) as u8, (key & 0x7F) as u8, 0))
            .chain(pedals.into_iter().map(|(channel, controller)| Message::new(0xB0 | channel, controller, 0)))
            .collect()
    }

    /// Forgets everything (the receiver released all input notes itself).
    pub fn clear(&mut self) {
        self.keys.clear();
        self.pedals.clear();
    }

    /// True when `source` holds no key and no pedal.
    pub fn is_idle(&self, source: i32) -> bool {
        self.keys.get(&source).is_none_or(BTreeSet::is_empty) && self.pedals.get(&source).is_none_or(BTreeSet::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(messages: Vec<Message>) -> Vec<[u8; 3]> {
        messages.into_iter().map(|message| [message.status, message.data1, message.data2]).collect()
    }

    #[test]
    fn an_unplugged_source_owes_one_note_off_and_one_pedal_up() {
        // A key and the pedal held on channel 2, a key pressed and released on channel 3.
        let mut held = HeldInputs::new();
        for message in [Message::new(0x91, 62, 90), Message::new(0xB1, 64, 127), Message::new(0x92, 70, 80), Message::new(0x82, 70, 0)] {
            held.track(7, &message);
        }
        assert!(!held.is_idle(7));
        assert_eq!(bytes(held.take_releases(7)), [[0x81, 62, 0], [0xB1, 64, 0]]);
        assert!(held.take_releases(7).is_empty(), "exactly once");
        assert!(held.is_idle(7));
    }

    #[test]
    fn releases_are_per_source_and_in_a_stable_order() {
        let mut held = HeldInputs::new();
        for (source, message) in [
            (1, Message::new(0x92, 64, 1)),
            (1, Message::new(0x90, 72, 1)),
            (1, Message::new(0x90, 60, 1)),
            (2, Message::new(0x90, 60, 1)),
            (1, Message::new(0xB3, 64, 64)),
            (1, Message::new(0xB0, 64, 127)),
            (1, Message::new(0x90, 72, 0)), // note-on with velocity zero releases
        ] {
            held.track(source, &message);
        }
        assert_eq!(bytes(held.take_releases(1)), [[0x80, 60, 0], [0x82, 64, 0], [0xB0, 64, 0], [0xB3, 64, 0]]);
        assert_eq!(bytes(held.take_releases(2)), [[0x80, 60, 0]]);
    }

    #[test]
    fn channel_mode_messages_and_pedal_up_clear_state() {
        let mut held = HeldInputs::new();
        for message in [Message::new(0x90, 60, 1), Message::new(0x91, 61, 1), Message::new(0xB0, 64, 100), Message::new(0xB0, 123, 0), Message::new(0xB0, 64, 0)] {
            held.track(1, &message);
        }
        assert_eq!(bytes(held.take_releases(1)), [[0x81, 61, 0]], "all-notes-off cleared channel 1 only; the pedal came up");
        // A pedal half way up was passed on with its value: it is still owed its release. So are
        // sostenuto and soft.
        for message in [Message::new(0xB0, 64, 127), Message::new(0xB0, 64, 20), Message::new(0xB2, 66, 127), Message::new(0xB2, 67, 1), Message::new(0xB2, 67, 0)] {
            held.track(1, &message);
        }
        assert_eq!(bytes(held.take_releases(1)), [[0xB0, 64, 0], [0xB2, 66, 0]]);
        held.track(1, &Message::new(0x90, 60, 1));
        held.track(1, &Message::new(0xB0, 120, 0));
        held.track(1, &Message::new(0xF8, 0, 0));
        held.track(1, &Message::new(0xC0, 60, 0));
        assert!(held.is_idle(1));
        held.track(1, &Message::new(0x90, 60, 1));
        held.clear();
        assert!(held.take_releases(1).is_empty());
    }
}

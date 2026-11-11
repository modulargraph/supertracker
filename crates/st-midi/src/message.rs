//! MIDI 1.0 messages and the byte-stream parser. Pure: no devices.

/// Number of data bytes that follow a status byte (0 for real-time and other one-byte messages).
pub fn data_length(status: u8) -> usize {
    if status < 0xF0 {
        return if status & 0xE0 == 0xC0 { 1 } else { 2 };
    }
    match status {
        0xF1 | 0xF3 => 1,
        0xF2 => 2,
        _ => 0,
    }
}

/// One short MIDI message: a status byte and up to two data bytes (unused ones are zero).
/// System-exclusive data is not represented; the parser skips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Message {
    /// Status byte, including the channel for channel messages (`0x90 | (channel - 1)`).
    pub status: u8,
    /// First data byte.
    pub data1: u8,
    /// Second data byte.
    pub data2: u8,
}

impl Message {
    /// A message from its bytes.
    pub const fn new(status: u8, data1: u8, data2: u8) -> Self {
        Message { status, data1, data2 }
    }

    /// The message kind: the status without its channel (`0x90` for a note-on), or the whole status
    /// for system messages.
    pub fn kind(&self) -> u8 {
        if self.status < 0xF0 {
            self.status & 0xF0
        } else {
            self.status
        }
    }

    /// The channel, 1 to 16, of a channel message.
    pub fn channel(&self) -> Option<u8> {
        (self.status >= 0x80 && self.status < 0xF0).then_some((self.status & 0x0F) + 1)
    }

    /// True for a note-on with a velocity above zero.
    pub fn is_note_on(&self) -> bool {
        self.kind() == 0x90 && self.data2 > 0
    }

    /// True for a note-off, including a note-on with velocity zero.
    pub fn is_note_off(&self) -> bool {
        self.kind() == 0x80 || (self.kind() == 0x90 && self.data2 == 0)
    }

    /// True when a device accepts this as a short message: a status byte that is not the start or
    /// end of system-exclusive data.
    pub fn is_sendable(&self) -> bool {
        self.status >= 0x80 && self.status != 0xF0 && self.status != 0xF7
    }

    /// The bytes to send and how many of them count. Data bytes are masked to seven bits.
    pub fn encode(&self) -> ([u8; 3], usize) {
        ([self.status, self.data1 & 0x7F, self.data2 & 0x7F], data_length(self.status) + 1)
    }
}

/// Turns a byte stream into messages.
///
/// Running status persists across calls; real-time bytes may occur inside any message and are
/// reported at once; system-exclusive data is skipped without mistaking its bytes for notes.
#[derive(Debug, Clone, Default)]
pub struct Parser {
    running_status: Option<u8>,
    status: Option<u8>,
    data: [u8; 2],
    count: usize,
    in_sysex: bool,
}

impl Parser {
    /// A parser with no running status.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one byte; returns the message it completes, if any.
    pub fn consume(&mut self, byte: u8) -> Option<Message> {
        if byte >= 0xF8 {
            return Some(Message::new(byte, 0, 0));
        }
        if byte >= 0x80 {
            self.count = 0;
            self.status = None;
            if byte == 0xF0 {
                self.in_sysex = true;
                self.running_status = None;
                return None;
            }
            if byte == 0xF7 {
                self.in_sysex = false;
                self.running_status = None;
                return None;
            }
            self.in_sysex = false;
            self.running_status = (byte < 0xF0).then_some(byte);
            if data_length(byte) == 0 {
                return Some(Message::new(byte, 0, 0));
            }
            self.status = Some(byte);
            return None;
        }
        if self.in_sysex {
            return None;
        }
        let current = self.status.or(self.running_status)?;
        self.status = Some(current);
        self.data[self.count] = byte;
        self.count += 1;
        if self.count < data_length(current) {
            return None;
        }
        let message = Message::new(current, self.data[0], if self.count > 1 { self.data[1] } else { 0 });
        self.count = 0;
        self.status = None;
        Some(message)
    }

    /// Feeds several bytes and hands every completed message to `each`.
    pub fn feed(&mut self, bytes: &[u8], mut each: impl FnMut(Message)) {
        for &byte in bytes {
            if let Some(message) = self.consume(byte) {
                each(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(parser: &mut Parser, bytes: &[u8]) -> Vec<[u8; 3]> {
        let mut out = Vec::new();
        parser.feed(bytes, |message| out.push([message.status, message.data1, message.data2]));
        out
    }

    #[test]
    fn running_status_and_realtime_bytes() {
        // Running status, a clock byte between messages, sustain, pitch bend and Stop.
        let mut parser = Parser::new();
        let input = [0x90, 60, 100, 61, 0, 0xF8, 0x90, 62, 90, 0xB0, 64, 127, 0xE0, 0, 64, 0xFC];
        assert_eq!(
            parse(&mut parser, &input),
            [[0x90, 60, 100], [0x90, 61, 0], [0xF8, 0, 0], [0x90, 62, 90], [0xB0, 64, 127], [0xE0, 0, 64], [0xFC, 0, 0]]
        );
    }

    #[test]
    fn running_status_persists_across_packets_and_realtime_interleaves() {
        let mut parser = Parser::new();
        assert_eq!(parse(&mut parser, &[0x91, 60]), Vec::<[u8; 3]>::new());
        assert_eq!(parse(&mut parser, &[0xF8, 100]), [[0xF8, 0, 0], [0x91, 60, 100]], "a clock byte inside a message does not disturb it");
        assert_eq!(parse(&mut parser, &[62, 0]), [[0x91, 62, 0]], "running status carries over");
        assert_eq!(parse(&mut parser, &[0xC2, 5, 6]), [[0xC2, 5, 0], [0xC2, 6, 0]], "one data byte for program change");
        assert_eq!(parse(&mut parser, &[0xD3, 9]), [[0xD3, 9, 0]]);
    }

    #[test]
    fn system_exclusive_is_skipped() {
        let mut parser = Parser::new();
        assert_eq!(parse(&mut parser, &[0x90, 60, 100, 0xF0, 0x7E, 60, 100, 0xF8, 1, 0xF7, 61, 100]), [[0x90, 60, 100], [0xF8, 0, 0]]);
        assert_eq!(parse(&mut parser, &[0x90, 61, 1]), [[0x90, 61, 1]], "running status does not survive system-exclusive data");
        // A status byte ends unterminated system-exclusive data.
        assert_eq!(parse(&mut parser, &[0xF0, 1, 2, 0x80, 61, 0]), [[0x80, 61, 0]]);
    }

    #[test]
    fn system_common_messages() {
        let mut parser = Parser::new();
        assert_eq!(parse(&mut parser, &[0xF2, 2, 3, 0xF3, 7, 0xF6, 0xFA]), [[0xF2, 2, 3], [0xF3, 7, 0], [0xF6, 0, 0], [0xFA, 0, 0]]);
        assert_eq!(parse(&mut parser, &[60, 100]), Vec::<[u8; 3]>::new(), "system common cancels running status");
        assert_eq!(parse(&mut parser, &[0x90, 1]), Vec::<[u8; 3]>::new());
        assert_eq!(parse(&mut parser, &[0x80, 2, 3]), [[0x80, 2, 3]], "a new status discards an unfinished message");
    }

    #[test]
    fn lengths_and_encoding() {
        assert_eq!([data_length(0x80), data_length(0x9F), data_length(0xA0), data_length(0xB0), data_length(0xE0)], [2; 5]);
        assert_eq!([data_length(0xC0), data_length(0xD5), data_length(0xF1), data_length(0xF3)], [1; 4]);
        assert_eq!([data_length(0xF8), data_length(0xFA), data_length(0xFC), data_length(0xF6), data_length(0xFE)], [0; 5]);
        assert_eq!(data_length(0xF2), 2);
        assert_eq!(Message::new(0x92, 200, 255).encode(), ([0x92, 200 & 0x7F, 0x7F], 3));
        assert_eq!(Message::new(0xC9, 64, 9).encode().1, 2);
        assert_eq!(Message::new(0xF8, 0, 0).encode().1, 1);
        assert!(!Message::new(0x40, 0, 0).is_sendable() && !Message::new(0xF0, 0, 0).is_sendable() && !Message::new(0xF7, 0, 0).is_sendable());
        assert!(Message::new(0xFA, 0, 0).is_sendable());
        let on = Message::new(0x93, 60, 1);
        assert!(on.is_note_on() && !on.is_note_off() && on.channel() == Some(4) && on.kind() == 0x90);
        assert!(Message::new(0x93, 60, 0).is_note_off() && Message::new(0x83, 60, 64).is_note_off());
        assert_eq!(Message::new(0xF8, 0, 0).channel(), None);
        assert_eq!(Message::new(0xF8, 0, 0).kind(), 0xF8);
    }
}

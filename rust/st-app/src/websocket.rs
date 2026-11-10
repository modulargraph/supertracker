//! The server side of the WebSocket protocol (RFC 6455), as much as the bridge needs: text messages
//! both ways, fragmented or not, ping/pong and close.
//!
//! It is written here instead of taken from a crate so that reading and writing can run on two
//! threads over one socket: the reader blocks in `read`, the writer blocks on the outbox, and
//! neither polls. The two share the socket's write half through [`Sender`], which writes whole
//! frames under a lock.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

/// Larger messages are refused (a project with every plug-in state stays far below this).
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// The value of `Sec-WebSocket-Accept` for a client's `Sec-WebSocket-Key`.
pub fn accept_key(client_key: &str) -> String {
    let mut input = client_key.trim().as_bytes().to_vec();
    input.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64(&sha1(&input))
}

fn sha1(message: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64) * 8).to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut words = [0u32; 80];
        for (index, bytes) in block.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..80 {
            words[index] = (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in words.iter().enumerate() {
            let (mix, constant) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let next = a.rotate_left(5).wrapping_add(mix).wrapping_add(e).wrapping_add(constant).wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (value, add) in state.iter_mut().zip([a, b, c, d, e]) {
            *value = value.wrapping_add(add);
        }
    }
    let mut digest = [0u8; 20];
    for (bytes, value) in digest.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&value.to_be_bytes());
    }
    digest
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16 | u32::from(*chunk.get(1).unwrap_or(&0)) << 8 | u32::from(*chunk.get(2).unwrap_or(&0));
        for position in 0..4 {
            if position <= chunk.len() {
                text.push(ALPHABET[(group >> (18 - 6 * position) & 63) as usize] as char);
            } else {
                text.push('=');
            }
        }
    }
    text
}

fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(payload.len() + 10);
    bytes.push(0x80 | opcode);
    match payload.len() {
        length @ 0..=125 => bytes.push(length as u8),
        length @ 126..=65535 => {
            bytes.push(126);
            bytes.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            bytes.push(127);
            bytes.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    bytes.extend_from_slice(payload);
    bytes
}

/// The write half of a connection, shared by the writer thread (messages) and the reader thread
/// (pongs and the closing handshake). Each call writes one whole frame.
pub struct Sender<W: Write> {
    stream: Arc<Mutex<W>>,
}

impl<W: Write> Clone for Sender<W> {
    fn clone(&self) -> Self {
        Sender { stream: self.stream.clone() }
    }
}

impl<W: Write> Sender<W> {
    pub fn new(stream: W) -> Self {
        Sender { stream: Arc::new(Mutex::new(stream)) }
    }

    fn write(&self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let bytes = frame(opcode, payload);
        let mut stream = self.stream.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        stream.write_all(&bytes)?;
        stream.flush()
    }

    pub fn text(&self, text: &str) -> io::Result<()> {
        self.write(OP_TEXT, text.as_bytes())
    }

    pub fn ping(&self) -> io::Result<()> {
        self.write(OP_PING, b"")
    }

    /// Sends a close frame with a status code (1000 normal, 1001 going away, 1009 too big, ...).
    pub fn close(&self, code: u16) -> io::Result<()> {
        self.write(OP_CLOSE, &code.to_be_bytes())
    }
}

/// What [`Receiver::next`] produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    Text(String),
    /// The peer closed the connection, or sent something that ends it. The closing frame has been
    /// answered.
    Closed,
}

/// The read half of a connection.
pub struct Receiver<R: Read, W: Write> {
    stream: R,
    sender: Sender<W>,
}

impl<R: Read, W: Write> Receiver<R, W> {
    pub fn new(stream: R, sender: Sender<W>) -> Self {
        Receiver { stream, sender }
    }

    fn fail(&self, code: u16) -> io::Result<Received> {
        let _ = self.sender.close(code);
        Ok(Received::Closed)
    }

    /// Blocks until the next text message. Pings are answered and pongs ignored on the way.
    pub fn next(&mut self) -> io::Result<Received> {
        let mut message: Vec<u8> = Vec::new();
        let mut in_message = false;
        loop {
            let mut head = [0u8; 2];
            self.stream.read_exact(&mut head)?;
            let (finished, opcode) = (head[0] & 0x80 != 0, head[0] & 0x0F);
            let (masked, short_length) = (head[1] & 0x80 != 0, u64::from(head[1] & 0x7F));
            // Reserved bits mean an extension nobody negotiated; clients must mask (RFC 6455, 5.1).
            if head[0] & 0x70 != 0 || !masked {
                return self.fail(1002);
            }
            let length = match short_length {
                126 => {
                    let mut bytes = [0u8; 2];
                    self.stream.read_exact(&mut bytes)?;
                    u64::from(u16::from_be_bytes(bytes))
                }
                127 => {
                    let mut bytes = [0u8; 8];
                    self.stream.read_exact(&mut bytes)?;
                    u64::from_be_bytes(bytes)
                }
                length => length,
            };
            let control = opcode >= 0x8;
            if (control && (length > 125 || !finished)) || length > (MAX_MESSAGE_BYTES - message.len().min(MAX_MESSAGE_BYTES)) as u64 {
                return self.fail(if control { 1002 } else { 1009 });
            }
            let mut mask = [0u8; 4];
            self.stream.read_exact(&mut mask)?;
            let start = if control { 0 } else { message.len() };
            let mut control_payload = Vec::new();
            let target = if control { &mut control_payload } else { &mut message };
            target.resize(start + length as usize, 0);
            self.stream.read_exact(&mut target[start..])?;
            for (index, byte) in target[start..].iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
            match opcode {
                OP_PING => self.sender.write(OP_PONG, &control_payload)?,
                OP_PONG => {}
                OP_CLOSE => {
                    let code = if control_payload.len() >= 2 { u16::from_be_bytes([control_payload[0], control_payload[1]]) } else { 1000 };
                    let _ = self.sender.close(code);
                    return Ok(Received::Closed);
                }
                OP_TEXT if !in_message => in_message = true,
                OP_CONTINUATION if in_message => {}
                // Binary messages are not part of the bridge.
                OP_BINARY => return self.fail(1003),
                _ => return self.fail(1002),
            }
            if !control && finished {
                return match String::from_utf8(std::mem::take(&mut message)) {
                    Ok(text) => Ok(Received::Text(text)),
                    Err(_) => self.fail(1007),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A frame as a browser sends it: masked.
    fn client_frame(finished: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x12, 0x34, 0x56, 0x78];
        let mut bytes = vec![if finished { 0x80 } else { 0 } | opcode];
        match payload.len() {
            length @ 0..=125 => bytes.push(0x80 | length as u8),
            length @ 126..=65535 => {
                bytes.push(0x80 | 126);
                bytes.extend_from_slice(&(length as u16).to_be_bytes());
            }
            length => {
                bytes.push(0x80 | 127);
                bytes.extend_from_slice(&(length as u64).to_be_bytes());
            }
        }
        bytes.extend_from_slice(&mask);
        bytes.extend(payload.iter().enumerate().map(|(index, byte)| byte ^ mask[index % 4]));
        bytes
    }

    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<u8>>>);

    impl Write for Written {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn receiver(input: Vec<u8>) -> (Receiver<Cursor<Vec<u8>>, Written>, Written) {
        let written = Written::default();
        (Receiver::new(Cursor::new(input), Sender::new(written.clone())), written)
    }

    #[test]
    fn the_accept_key_matches_the_rfc_example() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn sha1_and_base64_match_known_values() {
        let hex = |digest: [u8; 20]| digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(sha1(&[b'a'; 1000])), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
        for (plain, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn text_messages_of_every_length_form_are_read() {
        for length in [0usize, 5, 125, 126, 65535, 65536, 200_000] {
            let text: String = "é".repeat(length / 2) + if length % 2 == 1 { "x" } else { "" };
            let (mut receiver, _) = receiver(client_frame(true, OP_TEXT, text.as_bytes()));
            assert_eq!(receiver.next().unwrap(), Received::Text(text), "length {length}");
        }
    }

    #[test]
    fn fragments_are_joined_and_pings_between_them_answered() {
        let mut input = client_frame(false, OP_TEXT, b"{\"id\":1,");
        input.extend(client_frame(true, OP_PING, b"are you there"));
        input.extend(client_frame(false, OP_CONTINUATION, b"\"command\":"));
        input.extend(client_frame(true, OP_PONG, b""));
        input.extend(client_frame(true, OP_CONTINUATION, b"\"ready\"}"));
        input.extend(client_frame(true, OP_TEXT, b"next"));
        let (mut receiver, written) = receiver(input);
        assert_eq!(receiver.next().unwrap(), Received::Text("{\"id\":1,\"command\":\"ready\"}".to_string()));
        assert_eq!(receiver.next().unwrap(), Received::Text("next".to_string()));
        assert_eq!(*written.0.lock().unwrap(), frame(OP_PONG, b"are you there"));
    }

    #[test]
    fn a_close_frame_is_echoed_and_ends_the_connection() {
        let (mut receiver, written) = receiver(client_frame(true, OP_CLOSE, &1001u16.to_be_bytes()));
        assert_eq!(receiver.next().unwrap(), Received::Closed);
        assert_eq!(*written.0.lock().unwrap(), frame(OP_CLOSE, &1001u16.to_be_bytes()));
    }

    #[test]
    fn protocol_violations_close_with_the_matching_status() {
        let unmasked = frame(OP_TEXT, b"hello");
        let mut oversized = vec![0x80 | OP_TEXT, 0x80 | 127];
        oversized.extend_from_slice(&(MAX_MESSAGE_BYTES as u64 + 1).to_be_bytes());
        oversized.extend_from_slice(&[0; 4]);
        for (input, status) in [
            (unmasked, 1002u16),
            (client_frame(true, OP_BINARY, b"\x00\x01"), 1003),
            (client_frame(true, OP_TEXT, b"\xff\xfe"), 1007),
            (client_frame(true, OP_CONTINUATION, b"orphan"), 1002),
            (oversized, 1009),
        ] {
            let (mut receiver, written) = receiver(input);
            assert_eq!(receiver.next().unwrap(), Received::Closed);
            assert_eq!(*written.0.lock().unwrap(), frame(OP_CLOSE, &status.to_be_bytes()), "status {status}");
        }
    }

    #[test]
    fn a_connection_that_ends_mid_frame_is_an_error() {
        let mut input = client_frame(true, OP_TEXT, b"truncated");
        input.truncate(8);
        let (mut receiver, _) = receiver(input);
        assert!(receiver.next().is_err());
    }

    #[test]
    fn server_frames_are_unmasked_with_the_right_length_form() {
        assert_eq!(frame(OP_TEXT, b"hi"), [0x81, 2, b'h', b'i']);
        assert_eq!(&frame(OP_TEXT, &[b'a'; 300])[..4], [0x81, 126, 1, 44]);
        assert_eq!(&frame(OP_TEXT, &[b'a'; 70_000])[..10], [0x81, 127, 0, 0, 0, 0, 0, 1, 17, 112]);
    }
}

//! The wire format of the control socket and of MCP's stdio transport: one JSON-RPC 2.0 message per
//! line. JSON text never contains a raw newline, so a line feed ends a message.

use serde_json::{json, Map, Value};

use crate::ControlError;

/// The longest line either side accepts. A sender that exceeds it is cut off (the control socket) or
/// told so (MCP's standard input); nothing is ever buffered beyond this plus one read.
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Splits a byte stream into lines. The line feed is removed, and a carriage return before it;
/// empty lines are dropped. At most `limit` bytes of an unfinished line are kept.
#[derive(Debug)]
pub struct LineBuffer {
    pending: Vec<u8>,
    scanned: usize,
    limit: usize,
    overflowed: bool,
    discarding: bool,
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new(MAX_LINE_BYTES)
    }
}

impl LineBuffer {
    /// A buffer that accepts lines of up to `limit` bytes.
    pub fn new(limit: usize) -> Self {
        LineBuffer { pending: Vec::new(), scanned: 0, limit, overflowed: false, discarding: false }
    }

    /// Adds received bytes and returns the lines they complete.
    ///
    /// A line longer than the limit is thrown away and [`overflowed`](Self::overflowed) becomes
    /// true; when it is still unfinished, the rest of it is skipped up to its line feed.
    pub fn append(&mut self, mut data: &[u8]) -> Vec<Vec<u8>> {
        let mut lines = Vec::new();
        if self.discarding {
            match data.iter().position(|&byte| byte == b'\n') {
                Some(end) => {
                    self.discarding = false;
                    data = &data[end + 1..];
                }
                None => return lines,
            }
        }
        self.pending.extend_from_slice(data);
        let mut start = 0;
        while let Some(offset) = self.pending[self.scanned..].iter().position(|&byte| byte == b'\n') {
            let end = self.scanned + offset;
            let mut line = &self.pending[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            if line.len() > self.limit {
                self.overflowed = true;
            } else if !line.is_empty() {
                lines.push(line.to_vec());
            }
            start = end + 1;
            self.scanned = start;
        }
        self.pending.drain(..start);
        self.scanned = self.pending.len();
        if self.pending.len() > self.limit {
            self.overflowed = true;
            self.discarding = true;
            self.pending = Vec::new();
            self.scanned = 0;
        }
        lines
    }

    /// True once a line exceeded the limit. A socket reader closes the connection then.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Reads and resets the overflow mark (for a reader that reports the oversized line and goes on).
    pub fn take_overflow(&mut self) -> bool {
        std::mem::take(&mut self.overflowed)
    }

    /// The bytes of the unfinished line held right now.
    pub fn buffered(&self) -> usize {
        self.pending.len()
    }
}

/// One message as a line: compact JSON followed by a line feed.
pub fn encode(message: &Value) -> Vec<u8> {
    // serde_json escapes control characters inside strings and writes no newline between tokens.
    let mut line = serde_json::to_vec(message).unwrap_or_else(|_| b"{}".to_vec());
    line.push(b'\n');
    line
}

/// Reads one line as a JSON object.
pub fn decode(line: &[u8]) -> Result<Map<String, Value>, ControlError> {
    match serde_json::from_slice::<Value>(line) {
        Ok(Value::Object(message)) => Ok(message),
        _ => Err(ControlError::Protocol("Expected one JSON object per line.".into())),
    }
}

/// The named arguments of a request. A missing field means "no arguments"; anything present that
/// is not an object is an error, so that a malformed request can never turn into a valid call
/// without arguments (such as Undo).
pub fn named_object(value: Option<&Value>, field: &str) -> Result<Map<String, Value>, ControlError> {
    match value {
        None => Ok(Map::new()),
        Some(Value::Object(object)) => Ok(object.clone()),
        Some(_) => Err(ControlError::Protocol(format!("{field} must be a JSON object when provided."))),
    }
}

/// A JSON-RPC error response.
pub fn error_response(id: Option<&Value>, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_split_and_partial_lines_kept() {
        let mut buffer = LineBuffer::new(64);
        assert_eq!(buffer.append(b"{\"a\":1}\n{\"b\""), vec![b"{\"a\":1}".to_vec()]);
        assert_eq!(buffer.append(b":2}\r\n\n"), vec![b"{\"b\":2}".to_vec()]);
        assert_eq!(buffer.buffered(), 0);
    }

    #[test]
    fn an_unterminated_line_beyond_the_limit_is_rejected_and_skipped() {
        let mut buffer = LineBuffer::new(64);
        assert!(buffer.append(&[b'x'; 100]).is_empty());
        assert!(buffer.overflowed());
        assert_eq!(buffer.buffered(), 0);
        // The rest of the oversized line is not mistaken for a message.
        assert!(buffer.append(b"tail of it").is_empty());
        assert_eq!(buffer.append(b"end\n{\"ok\":1}\n"), vec![b"{\"ok\":1}".to_vec()]);
        assert!(buffer.take_overflow());
        assert!(!buffer.overflowed());
        // A complete oversized line in one piece is dropped as well.
        let mut whole = vec![b'y'; 65];
        whole.extend_from_slice(b"\n{\"next\":2}\n");
        assert_eq!(buffer.append(&whole), vec![b"{\"next\":2}".to_vec()]);
        assert!(buffer.take_overflow());
    }

    #[test]
    fn many_small_appends_do_not_rescan() {
        let mut buffer = LineBuffer::new(1 << 20);
        for _ in 0..10_000 {
            assert!(buffer.append(b"abcdefgh").is_empty());
        }
        let lines = buffer.append(b"\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 80_000);
    }

    #[test]
    fn encoded_messages_are_exactly_one_line() {
        let encoded = encode(&json!({"text": "line\nbreak", "path": "/a/b"}));
        assert_eq!(encoded.last(), Some(&b'\n'));
        assert!(!encoded[..encoded.len() - 1].contains(&b'\n'));
        assert!(String::from_utf8(encoded).unwrap().contains("/a/b"), "slashes stay readable");
    }

    #[test]
    fn only_objects_are_messages() {
        assert!(decode(b"[1]").is_err());
        assert!(decode(b"7").is_err());
        assert!(decode(b"{nope").is_err());
        assert_eq!(decode(b"{\"id\":1}").unwrap().get("id"), Some(&json!(1)));
    }

    #[test]
    fn named_arguments_must_be_objects() {
        assert!(named_object(None, "params").unwrap().is_empty());
        assert_eq!(named_object(Some(&json!({"a": 1})), "params").unwrap().get("a"), Some(&json!(1)));
        for malformed in [json!([]), json!("wrong"), Value::Null, json!(7), json!(true)] {
            let error = named_object(Some(&malformed), "arguments").unwrap_err();
            assert_eq!(error.to_string(), "arguments must be a JSON object when provided.");
        }
    }
}

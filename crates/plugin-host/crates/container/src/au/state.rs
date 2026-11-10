//! State framing: how an Audio Unit's `fullStateForDocument` dictionary travels as the
//! protocol's opaque state bytes. Platform independent (tested on Linux).
//!
//! ```text
//! offset  size
//!  0       4   magic "STAU"
//!  4       1   frame version (1)
//!  5       1   payload kind: 1 binary property list, 2 keyed archive (NSKeyedArchiver)
//!  6       2   flags, little-endian: bit 0 the dictionary came from `fullState` (the
//!              unit had no `fullStateForDocument`), bit 1 values that no encoding could
//!              carry were left out
//!  8      12   component type, subtype, manufacturer (big-endian four-character codes)
//! 20       8   payload length, little-endian
//! 28       n   payload
//! ```
//!
//! The payload is the dictionary serialised as a binary property list. A unit that puts
//! values a property list cannot hold into its state (the old host failed the whole save
//! on those) gets a keyed archive instead, which keeps them; only if that fails too are
//! the offending values dropped, and the frame says so.
//!
//! `set_state` also accepts a bare property list (binary or XML), which is what an
//! `.aupreset` file contains.
//!
//! The second half of this module, [`SemanticWriter`], gives a state dictionary a
//! canonical byte form (dictionary keys sorted, number types preserved). Two captures
//! with the same canonical form are the same state even when the serialiser ordered
//! their dictionaries differently; the backend then returns the earlier bytes, so that
//! an unchanged plug-in yields byte-identical state and the host can tell "nothing
//! changed" by comparing bytes (this replaces `AudioAUStateReuse.swift`).

use std::fmt;

use super::ident::{ComponentId, FourCc};

pub const MAGIC: [u8; 4] = *b"STAU";
pub const FRAME_VERSION: u8 = 1;
pub const HEADER_LEN: usize = 28;
/// Larger states are refused in both directions (the old host's limit).
pub const MAX_STATE_BYTES: usize = 128 * 1024 * 1024;

pub const FLAG_FROM_FULL_STATE: u16 = 1;
pub const FLAG_PRUNED: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    BinaryPlist = 1,
    KeyedArchive = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    Empty,
    TooLarge(usize),
    Truncated,
    UnsupportedVersion(u8),
    UnknownKind(u8),
    /// The bytes are neither a frame nor a property list.
    Unrecognised,
    /// The state was saved by a different Audio Unit.
    WrongComponent {
        saved: ComponentId,
        loading: ComponentId,
    },
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Empty => write!(f, "The saved Audio Unit state is empty."),
            StateError::TooLarge(size) => {
                write!(f, "The Audio Unit state is too large ({} MB).", size / (1024 * 1024))
            }
            StateError::Truncated => write!(f, "The saved Audio Unit state is incomplete."),
            StateError::UnsupportedVersion(version) => {
                write!(f, "The saved Audio Unit state was written by a newer version (format {version}).")
            }
            StateError::UnknownKind(kind) => write!(f, "The saved Audio Unit state has an unknown encoding ({kind})."),
            StateError::Unrecognised => write!(f, "The saved Audio Unit state is not a valid property list."),
            StateError::WrongComponent { saved, loading } => {
                write!(f, "This state was saved by another Audio Unit ({saved}), not by {loading}.")
            }
        }
    }
}
impl std::error::Error for StateError {}

/// A decoded frame; the payload borrows from the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub kind: PayloadKind,
    pub flags: u16,
    pub component: ComponentId,
    pub payload: &'a [u8],
}

pub fn encode_frame(
    kind: PayloadKind,
    flags: u16,
    component: ComponentId,
    payload: &[u8],
) -> Result<Vec<u8>, StateError> {
    if payload.len() > MAX_STATE_BYTES - HEADER_LEN {
        return Err(StateError::TooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(FRAME_VERSION);
    out.push(kind as u8);
    out.extend_from_slice(&flags.to_le_bytes());
    for code in [component.kind, component.subtype, component.manufacturer] {
        out.extend_from_slice(&code.bytes());
    }
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

pub fn decode_frame(data: &[u8]) -> Result<Frame<'_>, StateError> {
    if data.len() < HEADER_LEN || data[..4] != MAGIC {
        return Err(if data.len() >= 4 && data[..4] != MAGIC {
            StateError::Unrecognised
        } else {
            StateError::Truncated
        });
    }
    if data[4] != FRAME_VERSION {
        return Err(StateError::UnsupportedVersion(data[4]));
    }
    let kind = match data[5] {
        1 => PayloadKind::BinaryPlist,
        2 => PayloadKind::KeyedArchive,
        other => return Err(StateError::UnknownKind(other)),
    };
    let flags = u16::from_le_bytes([data[6], data[7]]);
    let code = |at: usize| FourCc::from_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    let component = ComponentId { kind: code(8), subtype: code(12), manufacturer: code(16) };
    let length = u64::from_le_bytes(data[20..28].try_into().unwrap());
    let payload = &data[HEADER_LEN..];
    if length != payload.len() as u64 {
        return Err(StateError::Truncated);
    }
    Ok(Frame { kind, flags, component, payload })
}

/// What `set_state` was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incoming<'a> {
    Framed(Frame<'a>),
    /// A bare property list (binary or XML), e.g. the contents of an `.aupreset` file.
    BarePlist(&'a [u8]),
}

/// Classifies and validates state bytes for the component that is about to load them.
pub fn classify(data: &[u8], loading: ComponentId) -> Result<Incoming<'_>, StateError> {
    if data.is_empty() {
        return Err(StateError::Empty);
    }
    if data.len() > MAX_STATE_BYTES {
        return Err(StateError::TooLarge(data.len()));
    }
    if data.starts_with(&MAGIC) {
        let frame = decode_frame(data)?;
        if frame.component != loading {
            return Err(StateError::WrongComponent { saved: frame.component, loading });
        }
        return Ok(Incoming::Framed(frame));
    }
    if data.starts_with(b"bplist") {
        return Ok(Incoming::BarePlist(data));
    }
    // XML: a UTF-8 byte order mark and white space may precede the first tag.
    let text = &data[..data.len().min(256)];
    let text = text.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(text);
    let start = text.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(text.len());
    let text = &text[start..];
    if [&b"<?xml"[..], b"<!DOCTYPE plist", b"<plist"].iter().any(|tag| text.starts_with(tag)) {
        return Ok(Incoming::BarePlist(data));
    }
    Err(StateError::Unrecognised)
}

// ---- canonical form -----------------------------------------------------------------------

/// Bounds of the canonical form; a state beyond them is simply never reused.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_bytes: 8 * 1024 * 1024, max_depth: 64, max_nodes: 100_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticError {
    /// A bound was exceeded.
    Limit,
    /// A value whose equality cannot be decided (a non-finite number, a foreign object).
    Unsupported,
}

/// Writes the canonical form of a property list. The caller walks the list and reports
/// what it finds; dictionaries must be reported with their keys sorted by
/// [`compare_keys`]. Number kinds are kept apart (the integer 1, the real 1.0 and `true`
/// are three different states: a plug-in can tell them apart after a restore).
#[derive(Debug)]
pub struct SemanticWriter {
    bytes: Vec<u8>,
    limits: Limits,
    nodes: usize,
}

impl SemanticWriter {
    pub fn new(limits: Limits) -> SemanticWriter {
        SemanticWriter { bytes: Vec::new(), limits, nodes: 0 }
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn push(&mut self, data: &[u8]) -> Result<(), SemanticError> {
        if data.len() > self.limits.max_bytes - self.bytes.len().min(self.limits.max_bytes) {
            return Err(SemanticError::Limit);
        }
        self.bytes.extend_from_slice(data);
        Ok(())
    }

    /// Call once per value (not per dictionary key) before writing it, with its depth
    /// (the root is 0).
    pub fn node(&mut self, depth: usize) -> Result<(), SemanticError> {
        self.nodes += 1;
        if depth > self.limits.max_depth || self.nodes > self.limits.max_nodes {
            return Err(SemanticError::Limit);
        }
        Ok(())
    }

    pub fn boolean(&mut self, value: bool) -> Result<(), SemanticError> {
        self.push(&[1, value as u8])
    }

    pub fn signed(&mut self, value: i64) -> Result<(), SemanticError> {
        self.push(&[2, if value < 0 { 1 } else { 0 }])?;
        self.push(&value.unsigned_abs().to_be_bytes())
    }

    /// An integer stored as unsigned 64-bit (values above `i64::MAX` exist).
    pub fn unsigned(&mut self, value: u64) -> Result<(), SemanticError> {
        self.push(&[2, 0])?;
        self.push(&value.to_be_bytes())
    }

    /// A real number; `storage` distinguishes the stored width (32 or 64 bits).
    pub fn real(&mut self, storage: u8, value: f64) -> Result<(), SemanticError> {
        if !value.is_finite() {
            return Err(SemanticError::Unsupported);
        }
        self.push(&[3, storage])?;
        self.push(&value.to_bits().to_be_bytes())
    }

    fn units(&mut self, units: &[u16]) -> Result<(), SemanticError> {
        if units.len() > self.limits.max_bytes / 2 {
            return Err(SemanticError::Limit);
        }
        self.push(&(units.len() as u64).to_be_bytes())?;
        if units.len() * 2 > self.limits.max_bytes - self.bytes.len().min(self.limits.max_bytes) {
            return Err(SemanticError::Limit);
        }
        self.bytes.reserve(units.len() * 2);
        for unit in units {
            self.bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Ok(())
    }

    /// A string as UTF-16 code units (compared unit by unit, without normalisation).
    pub fn string(&mut self, units: &[u16]) -> Result<(), SemanticError> {
        self.push(&[4])?;
        self.units(units)
    }

    pub fn data(&mut self, data: &[u8]) -> Result<(), SemanticError> {
        self.push(&[5])?;
        self.push(&(data.len() as u64).to_be_bytes())?;
        self.push(data)
    }

    /// A date as seconds since the reference date.
    pub fn date(&mut self, seconds: f64) -> Result<(), SemanticError> {
        if !seconds.is_finite() {
            return Err(SemanticError::Unsupported);
        }
        self.push(&[6])?;
        self.push(&seconds.to_bits().to_be_bytes())
    }

    /// Followed by `count` values.
    pub fn begin_array(&mut self, count: usize) -> Result<(), SemanticError> {
        self.push(&[7])?;
        self.push(&(count as u64).to_be_bytes())
    }

    /// Followed by `count` times [`SemanticWriter::key`] and a value, keys ascending.
    pub fn begin_dictionary(&mut self, count: usize) -> Result<(), SemanticError> {
        self.push(&[8])?;
        self.push(&(count as u64).to_be_bytes())
    }

    pub fn key(&mut self, units: &[u16]) -> Result<(), SemanticError> {
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return Err(SemanticError::Limit);
        }
        self.units(units)
    }
}

/// The order of dictionary keys in the canonical form: by UTF-16 code unit.
pub fn compare_keys(a: &[u16], b: &[u16]) -> std::cmp::Ordering {
    a.cmp(b)
}

/// Remembers the last capture of one instance.
#[derive(Debug, Default)]
pub struct Reuse {
    last: Option<(Vec<u8>, Vec<u8>)>,
}

impl Reuse {
    /// `fresh` is the state just captured, `semantic` its canonical form (`None` if it
    /// has none). Returns the bytes to hand to the host: the earlier capture's when the
    /// state is the same, else `fresh`.
    pub fn accept(&mut self, fresh: Vec<u8>, semantic: Option<Vec<u8>>) -> Vec<u8> {
        let Some(semantic) = semantic else {
            // Cannot prove anything about this state: never answer with an old one.
            self.last = None;
            return fresh;
        };
        if let Some((bytes, previous)) = &self.last {
            if *previous == semantic {
                return bytes.clone();
            }
        }
        self.last = Some((fresh.clone(), semantic));
        fresh
    }

    /// After a restore: the saved bytes become the reference, so that capturing an
    /// untouched plug-in returns exactly what was loaded. The next capture still reads
    /// the unit's real state and must prove equality.
    pub fn seed(&mut self, saved: &[u8], semantic: Option<Vec<u8>>) {
        self.last = semantic.map(|semantic| (saved.to_vec(), semantic));
    }

    pub fn clear(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delay() -> ComponentId {
        ComponentId::parse("aufx:dely:appl").unwrap()
    }

    #[test]
    fn frames_round_trip() {
        for (kind, flags, payload) in [
            (PayloadKind::BinaryPlist, 0u16, b"bplist00\xd0\x08\x00".to_vec()),
            (PayloadKind::KeyedArchive, FLAG_FROM_FULL_STATE | FLAG_PRUNED, vec![0u8; 100_000]),
            (PayloadKind::BinaryPlist, FLAG_FROM_FULL_STATE, Vec::new()),
        ] {
            let bytes = encode_frame(kind, flags, delay(), &payload).unwrap();
            assert_eq!(bytes.len(), HEADER_LEN + payload.len());
            assert_eq!(&bytes[..4], b"STAU");
            let frame = decode_frame(&bytes).unwrap();
            assert_eq!(frame, Frame { kind, flags, component: delay(), payload: &payload });
            assert_eq!(classify(&bytes, delay()), Ok(Incoming::Framed(frame)));
        }
    }

    #[test]
    fn the_header_layout_is_fixed() {
        let component = ComponentId::parse("aumu:dls%20:appl").unwrap();
        let bytes = encode_frame(PayloadKind::KeyedArchive, 0x0102, component, &[0xAA, 0xBB]).unwrap();
        let expected: Vec<u8> =
            [b"STAU".as_slice(), &[1, 2, 0x02, 0x01], b"aumudls appl", &[2, 0, 0, 0, 0, 0, 0, 0], &[0xAA, 0xBB]]
                .concat();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn damaged_frames_are_errors() {
        let bytes = encode_frame(PayloadKind::BinaryPlist, 0, delay(), b"payload").unwrap();
        for cut in 4..bytes.len() {
            assert_eq!(decode_frame(&bytes[..cut]), Err(StateError::Truncated), "cut {cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(decode_frame(&longer), Err(StateError::Truncated));
        let mut version = bytes.clone();
        version[4] = 9;
        assert_eq!(decode_frame(&version), Err(StateError::UnsupportedVersion(9)));
        let mut kind = bytes.clone();
        kind[5] = 3;
        assert_eq!(decode_frame(&kind), Err(StateError::UnknownKind(3)));
        // A forged length cannot cause a huge allocation or a panic.
        let mut forged = bytes.clone();
        forged[20..28].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode_frame(&forged), Err(StateError::Truncated));
        assert_eq!(decode_frame(b"nope, not a frame at all, really not"), Err(StateError::Unrecognised));
        assert_eq!(decode_frame(b"ST"), Err(StateError::Truncated));
    }

    #[test]
    fn a_state_of_another_unit_is_refused() {
        let bytes = encode_frame(PayloadKind::BinaryPlist, 0, delay(), b"x").unwrap();
        let other = ComponentId::parse("aufx:rvb2:appl").unwrap();
        let error = classify(&bytes, other).unwrap_err();
        assert_eq!(error, StateError::WrongComponent { saved: delay(), loading: other });
        assert!(error.to_string().contains("aufx:dely:appl"), "{error}");
    }

    #[test]
    fn bare_property_lists_are_recognised() {
        assert_eq!(classify(b"bplist00\xd0", delay()), Ok(Incoming::BarePlist(b"bplist00\xd0")));
        let xml = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict/></plist>";
        assert_eq!(classify(xml, delay()), Ok(Incoming::BarePlist(xml)));
        let with_bom = [&[0xEF, 0xBB, 0xBF][..], b"\n  <plist version=\"1.0\"><dict/></plist>"].concat();
        assert!(matches!(classify(&with_bom, delay()), Ok(Incoming::BarePlist(_))));
        assert_eq!(classify(b"", delay()), Err(StateError::Empty));
        assert_eq!(classify(b"   \n", delay()), Err(StateError::Unrecognised));
        assert_eq!(classify(b"{\"json\": true}", delay()), Err(StateError::Unrecognised));
        assert_eq!(classify(&[0u8; 64], delay()), Err(StateError::Unrecognised));
    }

    #[test]
    fn size_limits() {
        let huge = vec![0u8; MAX_STATE_BYTES + 1];
        assert_eq!(classify(&huge, delay()), Err(StateError::TooLarge(MAX_STATE_BYTES + 1)));
        assert!(matches!(encode_frame(PayloadKind::BinaryPlist, 0, delay(), &huge), Err(StateError::TooLarge(_))));
    }

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// A small property-list model, to drive the writer the way the macOS walker does.
    enum Value {
        Bool(bool),
        Int(i64),
        UInt(u64),
        Real(u8, f64),
        Text(&'static str),
        Data(Vec<u8>),
        Date(f64),
        Array(Vec<Value>),
        Dict(Vec<(&'static str, Value)>),
    }

    fn write(writer: &mut SemanticWriter, value: &Value, depth: usize) -> Result<(), SemanticError> {
        writer.node(depth)?;
        match value {
            Value::Bool(v) => writer.boolean(*v),
            Value::Int(v) => writer.signed(*v),
            Value::UInt(v) => writer.unsigned(*v),
            Value::Real(storage, v) => writer.real(*storage, *v),
            Value::Text(v) => writer.string(&utf16(v)),
            Value::Data(v) => writer.data(v),
            Value::Date(v) => writer.date(*v),
            Value::Array(items) => {
                writer.begin_array(items.len())?;
                items.iter().try_for_each(|item| write(writer, item, depth + 1))
            }
            Value::Dict(entries) => {
                let mut sorted: Vec<(Vec<u16>, &Value)> = entries.iter().map(|(k, v)| (utf16(k), v)).collect();
                sorted.sort_by(|a, b| compare_keys(&a.0, &b.0));
                writer.begin_dictionary(sorted.len())?;
                sorted.iter().try_for_each(|(key, item)| {
                    writer.key(key)?;
                    write(writer, item, depth + 1)
                })
            }
        }
    }

    fn canonical(value: &Value) -> Result<Vec<u8>, SemanticError> {
        let mut writer = SemanticWriter::new(Limits::default());
        write(&mut writer, value, 0)?;
        Ok(writer.finish())
    }

    #[test]
    fn dictionary_order_does_not_matter_but_everything_else_does() {
        let a = Value::Dict(vec![
            ("name", Value::Text("Init")),
            ("data", Value::Data(vec![1, 2, 3])),
            ("version", Value::Int(3)),
            ("nested", Value::Dict(vec![("x", Value::Real(64, 0.5)), ("y", Value::Bool(true))])),
        ]);
        let b = Value::Dict(vec![
            ("nested", Value::Dict(vec![("y", Value::Bool(true)), ("x", Value::Real(64, 0.5))])),
            ("version", Value::Int(3)),
            ("name", Value::Text("Init")),
            ("data", Value::Data(vec![1, 2, 3])),
        ]);
        assert_eq!(canonical(&a).unwrap(), canonical(&b).unwrap());

        let distinct = [
            Value::Int(1),
            Value::Int(-1),
            Value::Real(64, 1.0),
            Value::Real(32, 1.0),
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(0),
            Value::UInt(u64::MAX),
            Value::Text("1"),
            Value::Text(""),
            Value::Data(vec![1]),
            Value::Data(vec![]),
            Value::Date(1.0),
            Value::Array(vec![]),
            Value::Array(vec![Value::Int(1)]),
            Value::Array(vec![Value::Int(1), Value::Int(1)]),
            Value::Dict(vec![]),
            Value::Dict(vec![("a", Value::Int(1))]),
            Value::Dict(vec![("b", Value::Int(1))]),
            Value::Array(vec![Value::Text("a"), Value::Text("b")]),
            Value::Array(vec![Value::Text("b"), Value::Text("a")]),
            Value::Array(vec![Value::Text("ab")]),
            // Composed and decomposed "é" are different states.
            Value::Text("\u{e9}"),
            Value::Text("e\u{301}"),
            Value::Real(64, 0.0),
            Value::Real(64, -0.0),
        ];
        let forms: Vec<Vec<u8>> = distinct.iter().map(|v| canonical(v).unwrap()).collect();
        for i in 0..forms.len() {
            for j in 0..i {
                assert_ne!(forms[i], forms[j], "values {i} and {j} must differ");
            }
        }
        // Equal unsigned and signed integers are the same integer.
        assert_eq!(canonical(&Value::UInt(5)).unwrap(), canonical(&Value::Int(5)).unwrap());
        assert_eq!(canonical(&Value::Int(i64::MIN)).unwrap().len(), 10);
    }

    #[test]
    fn undecidable_and_oversized_states_have_no_canonical_form() {
        assert_eq!(canonical(&Value::Real(64, f64::NAN)), Err(SemanticError::Unsupported));
        assert_eq!(canonical(&Value::Array(vec![Value::Real(32, f64::INFINITY)])), Err(SemanticError::Unsupported));
        assert_eq!(canonical(&Value::Date(f64::NAN)), Err(SemanticError::Unsupported));

        let mut deep = Value::Int(1);
        for _ in 0..70 {
            deep = Value::Array(vec![deep]);
        }
        assert_eq!(canonical(&deep), Err(SemanticError::Limit));

        let limits = Limits { max_bytes: 64, max_depth: 8, max_nodes: 4 };
        let mut writer = SemanticWriter::new(limits);
        assert_eq!(write(&mut writer, &Value::Data(vec![0; 100]), 0), Err(SemanticError::Limit));
        let mut writer = SemanticWriter::new(limits);
        let many = Value::Array((0..10).map(Value::Int).collect());
        assert_eq!(write(&mut writer, &many, 0), Err(SemanticError::Limit));
        let mut writer = SemanticWriter::new(limits);
        let long = Value::Text("this string is far longer than the sixty-four bytes the writer may use");
        assert_eq!(write(&mut writer, &long, 0), Err(SemanticError::Limit));
        // Within the limits it works.
        let mut writer = SemanticWriter::new(limits);
        assert_eq!(write(&mut writer, &Value::Array(vec![Value::Int(1), Value::Int(2)]), 0), Ok(()));
    }

    #[test]
    fn reuse_returns_the_earlier_bytes_only_for_the_same_state() {
        let mut reuse = Reuse::default();
        let s1 = canonical(&Value::Dict(vec![("a", Value::Int(1))])).unwrap();
        let s2 = canonical(&Value::Dict(vec![("a", Value::Int(2))])).unwrap();
        assert_eq!(reuse.accept(b"first".to_vec(), Some(s1.clone())), b"first");
        // Same state, serialised differently: the first bytes come back.
        assert_eq!(reuse.accept(b"first-reordered".to_vec(), Some(s1.clone())), b"first");
        // Changed state: fresh bytes, which become the reference.
        assert_eq!(reuse.accept(b"second".to_vec(), Some(s2.clone())), b"second");
        assert_eq!(reuse.accept(b"second-again".to_vec(), Some(s2.clone())), b"second");
        // Back to the first state: fresh bytes again (only the last capture is kept).
        assert_eq!(reuse.accept(b"first-later".to_vec(), Some(s1.clone())), b"first-later");
        // A state without canonical form is never answered from memory, and forgets.
        assert_eq!(reuse.accept(b"opaque".to_vec(), None), b"opaque");
        assert_eq!(reuse.accept(b"after-opaque".to_vec(), Some(s1.clone())), b"after-opaque");
        // Seeding after a restore.
        reuse.seed(b"loaded", Some(s2.clone()));
        assert_eq!(reuse.accept(b"captured".to_vec(), Some(s2)), b"loaded");
        assert_eq!(reuse.accept(b"edited".to_vec(), Some(s1.clone())), b"edited");
        reuse.seed(b"loaded-opaque", None);
        assert_eq!(reuse.accept(b"fresh".to_vec(), Some(s1.clone())), b"fresh");
        reuse.clear();
        assert_eq!(reuse.accept(b"fresh-2".to_vec(), Some(s1)), b"fresh-2");
    }
}

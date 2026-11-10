//! Audio Unit identity: the `plugin_id` codec and the plain-data component description.
//! Platform independent (tested on Linux).
//!
//! # The `plugin_id` format
//!
//! ```text
//! <type>:<subtype>:<manufacturer>          e.g.  aufx:dely:appl   aumu:dls%20:appl
//! ```
//!
//! Each part is one four-character code (the `OSType`s of the component description),
//! written byte by byte, most significant byte first:
//!
//! * bytes `0x21..=0x7E` except `%` and `:` are written as that ASCII character;
//! * every other byte (space, control characters, `%`, `:`, and the bytes above `0x7E`
//!   that MacRoman codes use) is written as `%XX` with two upper-case hex digits.
//!
//! So a canonical id contains no spaces and nothing but printable ASCII, survives trimming
//! and JSON, and maps one to one to the three 32-bit codes. The decoder also accepts a
//! literal space where the canonical form has `%20` (`aumu:dls :appl`) and lower-case hex
//! digits; nothing else. Codes are case sensitive, as they are for the system.

use std::fmt;

/// A four-character code (`OSType`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FourCc(pub u32);

impl FourCc {
    pub const fn from_bytes(bytes: [u8; 4]) -> FourCc {
        FourCc(u32::from_be_bytes(bytes))
    }

    pub const fn bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// Parses one part of a `plugin_id`.
    pub fn parse(text: &str) -> Result<FourCc, IdError> {
        let mut bytes = [0u8; 4];
        let mut count = 0;
        let mut chars = text.bytes();
        while let Some(c) = chars.next() {
            let byte = match c {
                b'%' => {
                    let (Some(high), Some(low)) = (chars.next().and_then(hex), chars.next().and_then(hex)) else {
                        return Err(IdError::BadEscape(text.to_string()));
                    };
                    high << 4 | low
                }
                0x20..=0x7E if c != b':' => c,
                _ => return Err(IdError::BadCharacter(text.to_string())),
            };
            if count == 4 {
                return Err(IdError::BadLength(text.to_string()));
            }
            bytes[count] = byte;
            count += 1;
        }
        if count != 4 {
            return Err(IdError::BadLength(text.to_string()));
        }
        Ok(FourCc::from_bytes(bytes))
    }

    /// For people: the characters where they are printable ASCII, `?` elsewhere.
    pub fn display_lossy(self) -> String {
        self.bytes().iter().map(|&b| if (0x20..=0x7E).contains(&b) { b as char } else { '?' }).collect()
    }
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.bytes() {
            match byte {
                0x21..=0x7E if byte != b'%' && byte != b':' => write!(f, "{}", byte as char)?,
                _ => write!(f, "%{byte:02X}")?,
            }
        }
        Ok(())
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{self}'")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    /// Not three parts separated by `:`.
    BadShape(String),
    /// A part does not decode to exactly four bytes.
    BadLength(String),
    BadEscape(String),
    BadCharacter(String),
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::BadShape(id) => write!(
                f,
                "\"{id}\" is not an Audio Unit identifier (expected type:subtype:manufacturer, e.g. aufx:dely:appl)"
            ),
            IdError::BadLength(part) => {
                write!(f, "\"{part}\" is not a four-character code (write a space as %20)")
            }
            IdError::BadEscape(part) => write!(f, "\"{part}\" contains a % that is not followed by two hex digits"),
            IdError::BadCharacter(part) => {
                write!(f, "\"{part}\" contains a character that must be written as %XX")
            }
        }
    }
}
impl std::error::Error for IdError {}

pub const TYPE_MUSIC_DEVICE: FourCc = FourCc::from_bytes(*b"aumu");
pub const TYPE_EFFECT: FourCc = FourCc::from_bytes(*b"aufx");
pub const TYPE_MUSIC_EFFECT: FourCc = FourCc::from_bytes(*b"aumf");
pub const TYPE_MIDI_PROCESSOR: FourCc = FourCc::from_bytes(*b"aumi");
pub const TYPE_GENERATOR: FourCc = FourCc::from_bytes(*b"augn");

/// What a component is to the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    /// `aumu`: takes MIDI, produces audio.
    Instrument,
    /// `aufx`: audio in, audio out.
    Effect,
    /// `aumf`: an effect that also takes MIDI.
    MusicEffect,
    /// `aumi`: MIDI in, MIDI out.
    MidiEffect,
    /// `augn`: produces audio without input (hostable, not listed in the browser).
    Generator,
}

impl ComponentKind {
    pub fn from_type(kind: FourCc) -> Option<ComponentKind> {
        Some(match kind {
            TYPE_MUSIC_DEVICE => ComponentKind::Instrument,
            TYPE_EFFECT => ComponentKind::Effect,
            TYPE_MUSIC_EFFECT => ComponentKind::MusicEffect,
            TYPE_MIDI_PROCESSOR => ComponentKind::MidiEffect,
            TYPE_GENERATOR => ComponentKind::Generator,
            _ => return None,
        })
    }

    pub fn type_code(self) -> FourCc {
        match self {
            ComponentKind::Instrument => TYPE_MUSIC_DEVICE,
            ComponentKind::Effect => TYPE_EFFECT,
            ComponentKind::MusicEffect => TYPE_MUSIC_EFFECT,
            ComponentKind::MidiEffect => TYPE_MIDI_PROCESSOR,
            ComponentKind::Generator => TYPE_GENERATOR,
        }
    }

    /// Stable lower-case name for protocols and files.
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentKind::Instrument => "instrument",
            ComponentKind::Effect => "effect",
            ComponentKind::MusicEffect => "music-effect",
            ComponentKind::MidiEffect => "midi-effect",
            ComponentKind::Generator => "generator",
        }
    }

    /// Produces sound without an audio input (the host's fallback is silence, not dry).
    pub fn is_source(self) -> bool {
        matches!(self, ComponentKind::Instrument | ComponentKind::Generator)
    }

    /// The system gives these a MIDI input (`scheduleMIDIEventBlock`).
    pub fn takes_midi(self) -> bool {
        !matches!(self, ComponentKind::Effect | ComponentKind::Generator)
    }
}

/// The identity of an Audio Unit component: what `plugin_id` encodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComponentId {
    pub kind: FourCc,
    pub subtype: FourCc,
    pub manufacturer: FourCc,
}

impl ComponentId {
    pub fn parse(id: &str) -> Result<ComponentId, IdError> {
        let mut parts = id.split(':');
        let (Some(kind), Some(subtype), Some(manufacturer), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(IdError::BadShape(id.to_string()));
        };
        Ok(ComponentId {
            kind: FourCc::parse(kind)?,
            subtype: FourCc::parse(subtype)?,
            manufacturer: FourCc::parse(manufacturer)?,
        })
    }

    pub fn component_kind(&self) -> Option<ComponentKind> {
        ComponentKind::from_type(self.kind)
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.kind, self.subtype, self.manufacturer)
    }
}

/// One installed Audio Unit, as the plug-in browser needs it. Plain data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentInfo {
    /// The id to create an instance with (`format = "au"`).
    pub plugin_id: String,
    /// Name without the manufacturer ("AUDelay").
    pub name: String,
    pub manufacturer: String,
    pub kind: ComponentKind,
    /// The system's name of the type ("Effect", "Music Device", ...).
    pub type_name: String,
    /// Component version, `0xMMMMmmbb` (major, minor, bugfix).
    pub version: u32,
    /// "1.2.3".
    pub version_string: String,
    /// The unit ships its own editor view (else the generic parameter editor is shown).
    pub has_custom_view: bool,
    /// System and user tags (AVAudioUnitComponent `allTagNames`).
    pub tags: Vec<String>,
    pub has_midi_input: bool,
    pub has_midi_output: bool,
    /// An Audio Unit extension (version 3); else a version 2 component.
    pub is_v3: bool,
    pub sandbox_safe: bool,
}

/// `0x00010203` -> "1.2.3".
pub fn version_string(version: u32) -> String {
    format!("{}.{}.{}", version >> 16, (version >> 8) & 0xFF, version & 0xFF)
}

/// Sorts a listing the way the browser shows it and drops duplicate ids (the system can
/// list one component under several queries).
pub fn finish_listing(mut list: Vec<ComponentInfo>) -> Vec<ComponentInfo> {
    list.sort_by(|a, b| {
        let key = |c: &ComponentInfo| (c.manufacturer.to_lowercase(), c.name.to_lowercase(), c.plugin_id.clone());
        key(a).cmp(&key(b))
    });
    list.dedup_by(|a, b| a.plugin_id == b.plugin_id);
    list
}

// ---- presets ------------------------------------------------------------------------------

/// Bit set in a [`stpc_runtime::PresetInfo::id`] for user presets.
pub const PRESET_USER_BIT: u32 = 0x8000_0000;

/// Maps an `AUAudioUnitPreset.number` to the protocol's 32-bit preset id: factory presets
/// (number >= 0) keep their number; user presets (number < 0) become their magnitude with
/// [`PRESET_USER_BIT`] set. `None` if the number does not fit.
pub fn preset_id(number: i64) -> Option<u32> {
    if (0..PRESET_USER_BIT as i64).contains(&number) {
        Some(number as u32)
    } else if number < 0 && number > -(PRESET_USER_BIT as i64) {
        Some(PRESET_USER_BIT | (-number) as u32)
    } else {
        None
    }
}

/// Inverse of [`preset_id`].
pub fn preset_number(id: u32) -> i64 {
    if id & PRESET_USER_BIT != 0 {
        -((id & !PRESET_USER_BIT) as i64)
    } else {
        id as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_codes_round_trip() {
        let id = ComponentId::parse("aufx:dely:appl").unwrap();
        assert_eq!(id.kind, FourCc(0x6175_6678));
        assert_eq!(id.subtype, FourCc::from_bytes(*b"dely"));
        assert_eq!(id.manufacturer, FourCc(0x6170_706C));
        assert_eq!(id.to_string(), "aufx:dely:appl");
        assert_eq!(id.component_kind(), Some(ComponentKind::Effect));
    }

    #[test]
    fn spaces_are_escaped_and_accepted_literally() {
        let dls = ComponentId {
            kind: TYPE_MUSIC_DEVICE,
            subtype: FourCc::from_bytes(*b"dls "),
            manufacturer: FourCc::from_bytes(*b"appl"),
        };
        assert_eq!(dls.to_string(), "aumu:dls%20:appl");
        assert_eq!(ComponentId::parse("aumu:dls%20:appl").unwrap(), dls);
        assert_eq!(ComponentId::parse("aumu:dls :appl").unwrap(), dls);
        // Leading and all-space codes.
        let odd = FourCc::from_bytes(*b"  x ");
        assert_eq!(odd.to_string(), "%20%20x%20");
        assert_eq!(FourCc::parse(&odd.to_string()).unwrap(), odd);
        assert_eq!(FourCc::parse("    ").unwrap(), FourCc(0x2020_2020));
        // A trimmed literal form is an error, not another component.
        assert!(matches!(ComponentId::parse("aumu:dls:appl"), Err(IdError::BadLength(_))));
    }

    #[test]
    fn non_ascii_reserved_and_control_bytes() {
        // MacRoman "Åpp©": bytes above 0x7E.
        let code = FourCc::from_bytes([0x81, b'p', b'p', 0xA9]);
        assert_eq!(code.to_string(), "%81pp%A9");
        assert_eq!(FourCc::parse("%81pp%A9").unwrap(), code);
        assert_eq!(FourCc::parse("%81pp%a9").unwrap(), code);
        assert_eq!(code.display_lossy(), "?pp?");
        // The separators themselves, NUL and DEL.
        let code = FourCc::from_bytes([b':', b'%', 0, 0x7F]);
        assert_eq!(code.to_string(), "%3A%25%00%7F");
        assert_eq!(FourCc::parse("%3A%25%00%7F").unwrap(), code);
        let id = ComponentId { kind: TYPE_EFFECT, subtype: code, manufacturer: FourCc(0) };
        assert_eq!(id.to_string(), "aufx:%3A%25%00%7F:%00%00%00%00");
        assert_eq!(ComponentId::parse(&id.to_string()).unwrap(), id);
        // Raw non-ASCII text is ambiguous (which encoding?) and refused.
        assert!(matches!(FourCc::parse("Åpp1"), Err(IdError::BadCharacter(_))));
    }

    #[test]
    fn every_code_round_trips() {
        // All byte values in every position, plus a pseudo-random sweep.
        for byte in 0..=255u8 {
            for position in 0..4 {
                let mut bytes = *b"abcd";
                bytes[position] = byte;
                let code = FourCc::from_bytes(bytes);
                let text = code.to_string();
                assert!(text.bytes().all(|b| (0x21..=0x7E).contains(&b) && b != b':'), "{text}");
                assert_eq!(FourCc::parse(&text).unwrap(), code, "{text}");
            }
        }
        let mut x = 0x1234_5678u32;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let id = ComponentId { kind: FourCc(x), subtype: FourCc(x.rotate_left(7)), manufacturer: FourCc(!x) };
            assert_eq!(ComponentId::parse(&id.to_string()).unwrap(), id);
        }
    }

    #[test]
    fn malformed_ids_are_errors_with_readable_messages() {
        for bad in ["", "aufx", "aufx:dely", "aufx:dely:appl:x", "aufx::appl", "aufx:delay:appl", "aufx:del:appl"] {
            assert!(ComponentId::parse(bad).is_err(), "{bad}");
        }
        assert!(matches!(ComponentId::parse("aufx:de%2:appl"), Err(IdError::BadEscape(_))));
        assert!(matches!(ComponentId::parse("aufx:de%zz:appl"), Err(IdError::BadEscape(_))));
        assert!(matches!(ComponentId::parse("aufx:d\tly:appl"), Err(IdError::BadCharacter(_))));
        // The old host's decimal form is not an id here.
        assert!(ComponentId::parse("1635083896:1684368505:1634758764").is_err());
        let message = ComponentId::parse("nonsense").unwrap_err().to_string();
        assert!(message.contains("type:subtype:manufacturer"), "{message}");
    }

    #[test]
    fn kinds() {
        for kind in [
            ComponentKind::Instrument,
            ComponentKind::Effect,
            ComponentKind::MusicEffect,
            ComponentKind::MidiEffect,
            ComponentKind::Generator,
        ] {
            assert_eq!(ComponentKind::from_type(kind.type_code()), Some(kind));
        }
        assert_eq!(ComponentKind::from_type(FourCc::from_bytes(*b"auou")), None);
        assert!(ComponentKind::Instrument.is_source() && ComponentKind::Instrument.takes_midi());
        assert!(!ComponentKind::Effect.is_source() && !ComponentKind::Effect.takes_midi());
        assert!(ComponentKind::MusicEffect.takes_midi() && ComponentKind::MidiEffect.takes_midi());
    }

    #[test]
    fn versions_and_listing_order() {
        assert_eq!(version_string(0x0001_0203), "1.2.3");
        assert_eq!(version_string(0x0102_0000), "258.0.0");
        let info = |id: &str, name: &str, manufacturer: &str| ComponentInfo {
            plugin_id: id.into(),
            name: name.into(),
            manufacturer: manufacturer.into(),
            kind: ComponentKind::Effect,
            type_name: "Effect".into(),
            version: 0,
            version_string: "0.0.0".into(),
            has_custom_view: false,
            tags: vec![],
            has_midi_input: false,
            has_midi_output: false,
            is_v3: false,
            sandbox_safe: false,
        };
        let list = finish_listing(vec![
            info("aufx:bbbb:zzzz", "beta", "Zeta"),
            info("aufx:aaaa:appl", "AUDelay", "apple"),
            info("aufx:aaaa:appl", "AUDelay", "apple"),
            info("aufx:cccc:appl", "AUBandpass", "Apple"),
        ]);
        let ids: Vec<&str> = list.iter().map(|c| c.plugin_id.as_str()).collect();
        assert_eq!(ids, ["aufx:cccc:appl", "aufx:aaaa:appl", "aufx:bbbb:zzzz"]);
    }

    #[test]
    fn preset_ids() {
        assert_eq!(preset_id(0), Some(0));
        assert_eq!(preset_id(127), Some(127));
        assert_eq!(preset_id(-1), Some(PRESET_USER_BIT | 1));
        assert_eq!(preset_id(-42), Some(PRESET_USER_BIT | 42));
        assert_eq!(preset_id(i32::MAX as i64), Some(i32::MAX as u32));
        assert_eq!(preset_id(1 << 31), None);
        assert_eq!(preset_id(-(1 << 31)), None);
        assert_eq!(preset_id(i64::MIN), None);
        for number in [0i64, 1, 500, i32::MAX as i64, -1, -2, -500, -(i32::MAX as i64)] {
            assert_eq!(preset_number(preset_id(number).unwrap()), number);
        }
    }
}

//! A bounded reader for the object archive stored inside a Maschine group file (`.mxgrp`).
//!
//! A group file is a container whose payload is a portable binary object archive: integers are
//! a width byte followed by that many little-endian bytes, strings a length and UTF-8 bytes,
//! and every tracked object carries a numeric object ID. The archive holds a vector of sixteen
//! Sounds, each with mixer members, a name parameter and a chain of insert Slots that wrap
//! Modules (Sampler, drum synths, effects, hosted plug-ins), followed by the Group's own name
//! and inserts.
//!
//! The reader does not model every class. It locates records by their object IDs and class
//! registrations and validates each reading: a record is accepted only when exactly one
//! candidate decodes with consistent IDs and finite values. Object-ID distances depend on the
//! archive and Sound versions; the accepted versions are those written by Maschine 1.8 up to
//! Maschine 3 ([`SUPPORTED_ARCHIVE_VERSIONS`], Group versions 1..=23, the Sound versions of
//! [`sound_layout`]). Anything else is refused rather than guessed. Pad identity comes from
//! the declared sixteen-element Sound vector, never from sample file names.

use crate::error::{Error, Result};
use std::collections::BTreeMap;

/// Archive library versions written by Maschine 1.8 (10) up to Maschine 3 (19).
pub const SUPPORTED_ARCHIVE_VERSIONS: [u64; 9] = [10, 12, 13, 14, 15, 16, 17, 18, 19];
/// Pattern event classes registered alongside the Module classes.
const EVENT_CLASS_NAMES: [&str; 7] = ["NoteEvent", "MidiEvent", "AudioEvent", "FloatModulationEvent", "IntModulationEvent", "BoolModulationEvent", "EnumModulationEvent"];
const MAXIMUM_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn invalid(message: impl AsRef<str>) -> Error {
    Error::invalid(format!("Maschine group: {}", message.as_ref()))
}

/// One of the sixteen Sounds of a group.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    /// Pad position, 0..16.
    pub index: usize,
    /// The Sound's object ID.
    pub object_id: i64,
    /// Where its record starts.
    pub offset: usize,
    /// Object ID of its name parameter.
    pub name_object_id: i64,
    /// Where its name parameter starts.
    pub name_offset: usize,
    /// Its name (empty for an unnamed pad).
    pub name: String,
    /// Where the next Sound (or the Group's part) starts.
    pub end: usize,
}

/// A Module in an insert Slot.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// Object ID of the Slot.
    pub slot_object_id: i64,
    /// Object ID of the Module.
    pub object_id: i64,
    /// Where the Slot record starts.
    pub offset: usize,
    /// Where the Module's members start.
    pub body_offset: usize,
    /// Class name without its namespace ("Sampler", "Iceverb", "PluginHost" ...).
    pub kind: String,
    /// Class version.
    pub version: u64,
    /// Whether the Slot is active (not bypassed).
    pub enabled: bool,
    /// Where the Module's record ends.
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct ClassRecord {
    name: String,
    version: u64,
    definition_offset: usize,
    object_offset: usize,
}

/// Object-ID distances that depend on the Sound class version. Each was established from
/// installed libraries and is validated again per file: the name record must be a unique,
/// well-formed name parameter and every mixer member must decode in range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundLayout {
    /// Sound object ID to its name parameter, for the first Sound (which introduces tracked bases).
    pub first_name_delta: i64,
    /// The same for Sounds 2..=16.
    pub name_delta: i64,
    /// Shift of the mixer members relative to Sound version 6.
    pub channel_shift: i64,
}

/// The layout of a Sound version, for archives older than version 13 (`modern == false`) or newer.
pub fn sound_layout(sound_version: u64, modern: bool) -> Option<SoundLayout> {
    let layout = |first_name_delta, name_delta, channel_shift| Some(SoundLayout { first_name_delta, name_delta, channel_shift });
    match (sound_version, modern) {
        // Maschine 1.x: versions 1..=3 lack one routing member of version 6; versions 4 and 5
        // carry two further routing members that version 6 dropped.
        (1..=3, false) => layout(92, 89, -1),
        (4..=5, false) => layout(95, 92, 2),
        (6, false) => layout(93, 90, 0),
        // Maschine 2: one tracked base per parameter (four IDs per parameter instead of three).
        (6, true) => layout(119, 115, 0),
        // Versions 8 and 9 add the per-Sound note/MIDI parameter block before the name.
        (8..=9, true) => layout(643, 639, 0),
        _ => None,
    }
}

/// The archive integer encoding of `value`: a width byte and that many little-endian bytes.
pub fn encoded(value: i64) -> Vec<u8> {
    if value <= 0 {
        return vec![0];
    }
    let mut bytes = Vec::new();
    let mut rest = value as u64;
    while rest != 0 {
        bytes.push((rest & 255) as u8);
        rest >>= 8;
    }
    let mut result = vec![bytes.len() as u8];
    result.extend(bytes);
    result
}

/// Read-only access to archive bytes. Every read is bounds-checked; a range whose start lies
/// behind its end is empty.
#[derive(Clone, Copy)]
pub struct View<'a> {
    /// The file's bytes.
    pub bytes: &'a [u8],
}

pub(crate) struct Name {
    pub offset: usize,
    pub object_id: i64,
    pub value: String,
}

impl<'a> View<'a> {
    /// `count` (at most 8) little-endian bytes at `at`.
    pub fn little(&self, at: usize, count: usize) -> Option<u64> {
        if count > 8 {
            return None;
        }
        let bytes = self.bytes.get(at..at.checked_add(count)?)?;
        Some(bytes.iter().enumerate().fold(0u64, |value, (index, byte)| value | u64::from(*byte) << (8 * index)))
    }

    fn float(&self, at: usize) -> Option<f32> {
        self.little(at, 4).map(|bits| f32::from_bits(bits as u32))
    }

    /// An archive integer (at most four bytes, canonical) at `offset`, which advances past it.
    pub fn integer(&self, offset: &mut usize, limit: usize) -> Result<u64> {
        let limit = limit.min(self.bytes.len());
        if *offset >= limit {
            return Err(invalid("truncated integer."));
        }
        let width = self.bytes[*offset] as usize;
        let start = *offset + 1;
        if width > 4 || start + width > limit || (width > 0 && self.bytes[start + width - 1] == 0) {
            return Err(invalid("non-canonical or oversized archive integer."));
        }
        *offset = start + width;
        Ok(self.little(start, width).unwrap_or(0))
    }

    /// A signed archive integer: a negative width byte marks a negative value.
    pub fn signed_integer(&self, offset: &mut usize, limit: usize) -> Result<i64> {
        let limit = limit.min(self.bytes.len());
        if *offset >= limit {
            return Err(invalid("truncated signed integer."));
        }
        let marker = self.bytes[*offset] as i8;
        let width = marker.unsigned_abs() as usize;
        let start = *offset + 1;
        if width > 4 || start + width > limit || (width > 0 && self.bytes[start + width - 1] == 0) {
            return Err(invalid("non-canonical signed integer."));
        }
        *offset = start + width;
        let raw = self.little(start, width).unwrap_or(0) as i64;
        Ok(if marker < 0 { -raw } else { raw })
    }

    /// An archive string of at most `maximum` bytes at `offset`, which advances past it.
    pub fn string(&self, offset: &mut usize, limit: usize, maximum: usize) -> Result<String> {
        let mut at = *offset;
        let length = self.integer(&mut at, limit)? as usize;
        let limit = limit.min(self.bytes.len());
        if length > maximum || at + length > limit {
            return Err(invalid("invalid archive string."));
        }
        let text = std::str::from_utf8(&self.bytes[at..at + length]).ok().filter(|text| !text.contains('\0')).ok_or_else(|| invalid("invalid archive string."))?;
        *offset = at + length;
        Ok(text.to_string())
    }

    /// Every position in `start..end` where `pattern` occurs.
    pub fn matches(&self, pattern: &[u8], start: usize, end: usize) -> Vec<usize> {
        if pattern.is_empty() || end > self.bytes.len() || start > end || end - start < pattern.len() {
            return Vec::new();
        }
        (start..=end - pattern.len()).filter(|at| self.bytes[*at] == pattern[0] && self.bytes[*at..*at + pattern.len()] == *pattern).collect()
    }

    /// Name parameters in `start..end`: those with the object ID `object_id`, or any.
    pub(crate) fn names(&self, object_id: Option<i64>, start: usize, end: usize, modern: bool, strict_trailer: bool) -> Vec<Name> {
        let end = end.min(self.bytes.len());
        if start > end {
            return Vec::new();
        }
        let candidates: Vec<usize> = match object_id {
            Some(id) => self.matches(&encoded(id), start, end),
            None => (start..end).collect(),
        };
        let mut found = Vec::new();
        for candidate in candidates {
            let mut at = candidate;
            let Ok(id) = self.integer(&mut at, end) else { continue };
            let id = id as i64;
            if id <= 0 || object_id.is_some_and(|wanted| wanted != id) {
                continue;
            }
            // The first name parameter registers two untracked bases (two zero bytes). An empty
            // name also begins with zero bytes, so both readings are tried; the tracked IDs and
            // the value block that follow let at most one of them validate.
            let skips: &[usize] = if at + 2 <= end && self.bytes[at] == 0 && self.bytes[at + 1] == 0 { &[2, 0] } else { &[0] };
            for skip in skips {
                if let Some(value) = self.parameter_name(id, at + skip, end, modern, strict_trailer) {
                    found.push(Name { offset: candidate, object_id: id, value });
                    break;
                }
            }
        }
        found
    }

    fn parameter_name(&self, id: i64, start: usize, end: usize, modern: bool, strict_trailer: bool) -> Option<String> {
        let mut at = start;
        if modern {
            if at >= end || self.bytes[at] != 0 {
                return None;
            }
            at += 1;
        }
        let name = self.string(&mut at, end, 128).ok().filter(|name| !name.chars().any(char::is_control))?;
        let width = if modern { 4 } else { 3 };
        for n in 1..=width {
            if self.integer(&mut at, end).ok()? as i64 != id + n {
                return None;
            }
        }
        if at + if modern { 18 } else { 17 } > end {
            return None;
        }
        let first = at + usize::from(modern);
        if modern && self.bytes[at] != 0 {
            return None;
        }
        if !self.float(first)?.is_finite() {
            return None;
        }
        if strict_trailer {
            if !self.bytes[first + 12..first + 17].iter().all(|byte| *byte == 0) {
                return None;
            }
        } else if !(1..=2).all(|n| self.float(first + 4 * n).is_some_and(f32::is_finite)) || self.bytes[first + 12] > 1 {
            // A few groups store data in the name parameter's trailing block: then all three
            // floats must be finite and the flag 0 or 1.
            return None;
        }
        Some(name)
    }
}

/// A decoded group archive.
#[derive(Debug, Clone, PartialEq)]
pub struct Archive {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// Version of the archive library.
    pub archive_version: u64,
    /// Version of the Group class.
    pub group_version: u64,
    /// Version of the Sound class.
    pub sound_version: u64,
    /// The sixteen Sounds in pad order.
    pub sounds: Vec<Sound>,
    /// Every Module in file order. Empty for a group without any insert Slot (MIDI-only Sounds).
    pub modules: Vec<Module>,
    /// Whether the archive registers a Slot class.
    pub has_slots: bool,
    /// Where the Group's name parameter starts.
    pub group_name_offset: usize,
    /// The Group's name.
    pub group_name: String,
    /// The object-ID layout of the Sound version.
    pub sound_layout: SoundLayout,
}

impl Archive {
    /// Whether the archive uses the Maschine 2+ parameter form (four IDs per parameter).
    pub fn modern(&self) -> bool {
        self.archive_version >= 13
    }

    /// A view of the bytes.
    pub fn view(&self) -> View<'_> {
        View { bytes: &self.bytes }
    }

    /// The Modules of a Sound, in chain order.
    pub fn modules_of(&self, sound: &Sound) -> Vec<&Module> {
        self.modules.iter().filter(|module| sound.offset <= module.offset && module.offset < sound.end).collect()
    }

    /// The Group's own inserts.
    pub fn group_modules(&self) -> Vec<&Module> {
        self.modules.iter().filter(|module| module.offset > self.group_name_offset).collect()
    }

    /// Decodes a group file. Fails for anything that is not an unambiguous archive of a
    /// supported version; never panics on damaged input.
    pub fn parse(data: &[u8]) -> Result<Archive> {
        if data.len() < 256 || data.len() > MAXIMUM_BYTES {
            return Err(invalid("the file is empty, truncated, or larger than 32 MB."));
        }
        let view = View { bytes: data };
        if view.little(0, 8) != Some(data.len() as u64) || &data[12..16] != b"hsin" || view.matches(b"2SAM PRG", 0, data.len().min(4096)).is_empty() {
            return Err(invalid("this is not a supported native .mxgrp container."));
        }
        const SIGNATURE: &[u8] = b"serialization::archive";
        let envelope = || invalid("the portable archive envelope is invalid or ambiguous.");
        let signatures = view.matches(SIGNATURE, 0, data.len());
        let magic = match signatures.as_slice() {
            [magic] if *magic >= 6 && data[magic - 2] == 1 && data[magic - 1] as usize == SIGNATURE.len() => *magic,
            _ => return Err(envelope()),
        };
        let count = view.little(magic - 6, 4).ok_or_else(envelope)? as usize;
        if count < 32 || count > data.len() - magic + 2 {
            return Err(envelope());
        }
        let stop = magic - 2 + count;
        let mut position = magic + SIGNATURE.len();
        let archive_version = view.integer(&mut position, stop)?;
        if view.integer(&mut position, stop)? != 0 || view.integer(&mut position, stop)? != 1 {
            return Err(invalid("the archive byte order or tracking mode is unsupported."));
        }
        let group_version = view.integer(&mut position, stop)?;
        if !SUPPORTED_ARCHIVE_VERSIONS.contains(&archive_version) || !(1..=23).contains(&group_version) {
            return Err(invalid(format!("archive {archive_version}, Group {group_version} has not been decoded safely yet.")));
        }
        let modern = archive_version >= 13;
        let next = |cursor: &mut usize| view.integer(cursor, stop).ok();

        // The vector of sixteen shared Sound pointers: count 16, item version 1, the pointer
        // class introduction, then the Sound class and its version.
        const VECTOR_PREFIX: [u8; 7] = [1, 16, 1, 1, 0, 1, 1];
        let mut headers: Vec<(usize, i64, u64)> = Vec::new();
        for offset in view.matches(&VECTOR_PREFIX, position, stop) {
            let mut cursor = offset + VECTOR_PREFIX.len();
            let Some(class_id) = next(&mut cursor).filter(|id| (1..=255).contains(id)) else { continue };
            if next(&mut cursor) != Some(1) {
                continue;
            }
            let Some(version) = next(&mut cursor).filter(|version| sound_layout(*version, modern).is_some()) else { continue };
            let Some(object) = next(&mut cursor).filter(|object| *object > 0) else { continue };
            if next(&mut cursor) != Some(2) || next(&mut cursor) != Some(object + 1) {
                continue;
            }
            headers.push((offset + VECTOR_PREFIX.len(), class_id as i64, version));
        }
        let &[(first_record, sound_class, sound_version)] = headers.as_slice() else {
            return Err(invalid("the sixteen-Sound vector cannot be identified unambiguously."));
        };
        let class_bytes = encoded(sound_class);
        let mut records: Vec<(usize, i64)> = Vec::new();
        for offset in view.matches(&class_bytes, position, stop) {
            let mut cursor = offset + class_bytes.len();
            if offset == first_record && (next(&mut cursor) != Some(1) || next(&mut cursor) != Some(sound_version)) {
                continue;
            }
            let Some(object) = next(&mut cursor).filter(|object| *object > 0) else { continue };
            if next(&mut cursor) != Some(2) || next(&mut cursor) != Some(object + 1) {
                continue;
            }
            records.push((offset, object as i64));
        }
        if records.len() != 16 || records[0].0 != first_record || !records.windows(2).all(|pair| pair[0].1 < pair[1].1) {
            return Err(invalid("Sound object identities are incomplete or ambiguous."));
        }
        let layout = sound_layout(sound_version, modern).ok_or_else(|| invalid(format!("Sound version {sound_version} has not been decoded safely yet.")))?;
        let mut sounds: Vec<Sound> = Vec::with_capacity(16);
        for (index, record) in records.iter().enumerate() {
            let end = records.get(index + 1).map(|next| next.0).unwrap_or(stop);
            // Each Sound version has a fixed sequence of members before its name. The first
            // Sound also introduces tracked objects that later Sounds normally reference by ID,
            // so its name sits a few object IDs further on. Some groups (external-MIDI kits)
            // give every Sound its own copy instead; exactly one of the two readings must validate.
            let deltas: &[i64] = if index == 0 { &[layout.first_name_delta] } else { &[layout.name_delta, layout.first_name_delta] };
            let mut candidates: Vec<Name> = deltas.iter().flat_map(|delta| view.names(Some(record.1 + delta), record.0, end.min(record.0 + 8192), modern, true)).collect();
            if candidates.len() != 1 {
                return Err(invalid(format!("Sound {}'s name/member layout is unsupported.", index + 1)));
            }
            let name = candidates.remove(0);
            sounds.push(Sound { index, object_id: record.1, offset: record.0, name_object_id: name.object_id, name_offset: name.offset, name: name.value, end });
        }
        // One further mixer layer, owned by the Group, follows Sound 16. Its member shape, not
        // its display name, determines the boundary.
        let boundary = || invalid("the Group/Sound effect boundary is not unambiguous.");
        let last = &sounds[15];
        let group_start = last.name_offset + last.name.len() + 32;
        if group_start > stop {
            return Err(boundary());
        }
        let last_name_id = last.name_object_id;
        let after_last = |strict: bool| -> Vec<Name> { view.names(None, group_start, stop, modern, strict).into_iter().filter(|name| name.object_id > last_name_id).collect() };
        let mut group_names = after_last(true);
        if group_names.is_empty() {
            group_names = after_last(false);
        }
        if group_names.len() != 1 {
            return Err(boundary());
        }
        let group_name = group_names.remove(0);
        sounds[15].end = group_name.offset;

        // Module classes are registered by their qualified name where they first occur.
        const TYPE_PREFIX: &[u8] = b"NI::MASCHINE::DATA::";
        let mut classes: BTreeMap<u64, ClassRecord> = BTreeMap::new();
        for offset in view.matches(TYPE_PREFIX, position, stop) {
            if offset < 4 || data[offset - 2] != 1 {
                continue;
            }
            let length = data[offset - 1] as usize;
            if length < TYPE_PREFIX.len() || length > 128 || offset + length >= stop {
                continue;
            }
            let full = &data[offset..offset + length];
            if !full.iter().all(|byte| *byte == b':' || *byte == b'_' || byte.is_ascii_alphanumeric()) {
                continue;
            }
            let mut cursor = offset - 4;
            let Some(id) = view.integer(&mut cursor, offset - 2).ok().filter(|_| cursor == offset - 2) else { continue };
            cursor = offset + length;
            if next(&mut cursor) != Some(1) {
                continue;
            }
            let Some(version) = next(&mut cursor).filter(|version| *version <= 32) else { continue };
            let record = ClassRecord { name: String::from_utf8_lossy(&full[TYPE_PREFIX.len()..]).into_owned(), version, definition_offset: offset - 4, object_offset: cursor };
            if classes.insert(id, record).is_some() {
                return Err(invalid("duplicate module class registration."));
            }
        }
        // The Slot class wraps an enable parameter and one Module. It is declared with the first
        // Slot of the archive: in the first Sound (or the Group) that has an insert chain, where
        // the first Slot directly follows the name.
        let mut owners: Vec<(i64, usize, usize)> = sounds.iter().map(|sound| (sound.name_object_id, sound.name_offset, sound.end)).collect();
        owners.push((group_name.object_id, group_name.offset, stop));
        let inherited: i64 = if modern { 4 } else { 3 };
        let mut declaration: Option<(usize, u64, i64)> = None;
        for (name_id, name_offset, owner_end) in owners {
            let first_slot = name_id + if modern { 18 } else { 14 };
            let sequence: Vec<u8> = (first_slot..=first_slot + inherited).flat_map(encoded).collect();
            let hits = view.matches(&sequence, name_offset, owner_end);
            match hits.as_slice() {
                [hit] if *hit >= 5 => {
                    let mut cursor = hit - 5;
                    let class = view.integer(&mut cursor, *hit).ok();
                    let valid = class.is_some() && view.integer(&mut cursor, *hit).ok() == Some(1) && view.integer(&mut cursor, *hit).ok() == Some(0) && cursor == *hit;
                    if !valid {
                        return Err(invalid("the Slot class declaration is invalid."));
                    }
                    declaration = Some((*hit, class.unwrap_or(0), first_slot));
                    break;
                }
                [] => continue,
                _ => return Err(invalid("the plug-in Slot layout is unsupported.")),
            }
        }
        let mut archive = Archive {
            bytes: data.to_vec(),
            archive_version,
            group_version,
            sound_version,
            sounds,
            modules: Vec::new(),
            has_slots: declaration.is_some(),
            group_name_offset: group_name.offset,
            group_name: group_name.value,
            sound_layout: layout,
        };
        let Some((first_slot_offset, slot_class, first_slot)) = declaration else {
            // MIDI-only groups (external hardware templates, input Sounds) have no Slot at all.
            // That is accepted only when no Module class is registered anywhere.
            if classes.values().any(|class| !EVENT_CLASS_NAMES.contains(&class.name.as_str())) {
                return Err(invalid("the plug-in Slot layout is unsupported."));
            }
            return Ok(archive);
        };
        let slot_bytes = encoded(slot_class as i64);
        let mut modules: Vec<Module> = Vec::new();
        for offset in view.matches(&slot_bytes, first_slot_offset - 5, stop) {
            let mut cursor = offset + slot_bytes.len();
            if cursor + 3 == first_slot_offset {
                cursor = first_slot_offset;
            }
            let Some(slot_id) = next(&mut cursor).map(|id| id as i64).filter(|id| *id >= first_slot) else { continue };
            if !(1..=inherited).all(|n| next(&mut cursor) == Some((slot_id + n) as u64)) {
                continue;
            }
            if modern {
                if cursor >= stop || data[cursor] != 0 {
                    continue;
                }
                cursor += 1;
            }
            let Some(enabled) = next(&mut cursor).filter(|enabled| *enabled <= 1) else { continue };
            if !(0..4).all(|_| next(&mut cursor).is_some_and(|value| value <= 1)) {
                continue;
            }
            // The first shared Module pointer introduces the pointer class.
            if cursor + 3 <= stop && data[cursor..cursor + 3] == [0, 1, 1] {
                cursor += 3;
            }
            let class_offset = cursor;
            let Some(class) = next(&mut cursor).and_then(|id| classes.get(&id)) else { continue };
            if class_offset == class.definition_offset {
                cursor = class.object_offset;
            }
            if next(&mut cursor) != Some((slot_id + inherited + 1) as u64) {
                continue;
            }
            modules.push(Module { slot_object_id: slot_id, object_id: slot_id + inherited + 1, offset, body_offset: cursor, kind: class.name.clone(), version: class.version, enabled: enabled == 1, end: stop });
        }
        if modules.is_empty() {
            return Err(invalid("no supported Module records were found."));
        }
        for index in 0..modules.len() {
            let own_boundary = archive.sounds.iter().find(|sound| sound.offset <= modules[index].offset && modules[index].offset < sound.end).map(|sound| sound.end).unwrap_or(stop);
            let next_module = modules.get(index + 1).map(|module| module.offset).unwrap_or(stop);
            modules[index].end = own_boundary.min(next_module).max(modules[index].body_offset.min(stop));
        }
        archive.modules = modules;
        Ok(archive)
    }

    // -----------------------------------------------------------------------------------------
    // Members

    /// The value of the numeric parameter whose last tracked object ID is `final_id`, looked for
    /// in `start..end`. The stored value is a float, or a signed integer or enum, depending on
    /// the parameter class.
    pub fn scalar(&self, final_id: i64, float: bool, start: usize, end: usize) -> Result<f64> {
        let view = self.view();
        let end = end.min(self.bytes.len());
        let mut prefix = encoded(final_id - 1);
        prefix.extend(encoded(final_id));
        let mut values: Vec<f64> = Vec::new();
        for offset in view.matches(&prefix, start, end) {
            let mut cursor = offset + prefix.len();
            if self.modern() {
                if cursor >= end || self.bytes[cursor] != 0 {
                    continue;
                }
                cursor += 1;
            }
            if float {
                if cursor + 17 > end {
                    continue;
                }
                let finite = |at: usize| view.float(at).filter(|value| value.is_finite());
                let (Some(value), Some(_), Some(_)) = (finite(cursor), finite(cursor + 4), finite(cursor + 8)) else { continue };
                let mut tail = cursor + 12;
                if !view.integer(&mut tail, end).is_ok_and(|flag| flag <= 1) || tail + 4 > end || finite(tail).is_none() {
                    continue;
                }
                values.push(f64::from(value));
            } else {
                let Ok(value) = view.signed_integer(&mut cursor, end) else { continue };
                if (0..4).all(|_| view.signed_integer(&mut cursor, end).is_ok()) {
                    values.push(value as f64);
                }
            }
        }
        match values.as_slice() {
            [value] => Ok(*value),
            _ => Err(invalid(format!("parameter object {final_id} is missing or ambiguous."))),
        }
    }

    /// The numeric members of an effect or drum-synth Module, in serializer order (which is not
    /// the order of the controls in Maschine's interface). `e` members are enums, `b` booleans,
    /// `i` integers, `f` floats.
    pub fn parameters(&self, module: &Module) -> Result<Vec<f64>> {
        let modern = self.modern();
        let (wide, narrow): (i64, i64) = if modern { (5, 4) } else { (4, 3) };
        let (start, end) = (module.body_offset, module.end);
        if module.kind == "Limiter" && module.version == 1 {
            let mut result = vec![self.scalar(module.object_id + narrow, true, start, end)?];
            // The side-chain filter holds one source object, three numeric controls and one
            // boolean; every member is validated before this block is crossed.
            let mut id = module.object_id + narrow + 1;
            for float in [true, true, true, false] {
                id += narrow;
                self.scalar(id, float, start, end)?;
            }
            id += wide;
            result.push(self.scalar(id, false, start, end)?);
            for _ in 0..2 {
                id += narrow;
                result.push(self.scalar(id, true, start, end)?);
            }
            return Ok(result);
        }
        let schema: Option<(&[u64], &str)> = match module.kind.as_str() {
            "BeatDelay" => Some((&[0], "eeffffffe")),
            "CabinetEmulation" => Some((&[0], "eeff")),
            "Chorus" => Some((&[0], "fff")),
            "Compressor" => Some((&[1], "effffff")),
            "Cymbal" => Some((&[0], "efffffffeffff")),
            "EQ" => Some((&[0], "fffffffffff")),
            "FM" => Some((&[0], "ffff")),
            "Filter" => Some((&[0], "efffeffffff")),
            "Flanger" => Some((&[0], "ffffffbf")),
            "FreqShifter" => Some((&[0], "ffffbf")),
            "Gate" => Some((&[0], "fffff")),
            "GrainDelay" => Some((&[0], "fffbfffff")),
            "GrainStretch" => Some((&[0], "bfefbff")),
            "Hihat" => Some((&[2], "efffbffffffeffffff")),
            "Iceverb" => Some((&[0], "ffff")),
            "Kick" => Some((&[2], "effffffefffffffffffeffeffeffbff")),
            "LoFi" => Some((&[0], "fffff")),
            "Maximizer" => Some((&[0], "ffb")),
            "Metaverb" => Some((&[0], "fffff")),
            "MullhollandDrive" => Some((&[1], "efffbfffffffbff")),
            "Perc" => Some((&[1], "effffeffffffffbeffeeffefeffff")),
            "Phaser" => Some((&[0], "ffeffffbf")),
            "PlateReverb" => Some((&[1], "fffffefffffffffffffffff")),
            "Reflex" => Some((&[0], "ffff")),
            "Resochord" => Some((&[0], "eeefiffff")),
            "Reverb" => Some((&[0], "effffffb")),
            "Saturator" => Some((&[0], "efffffffffffbb")),
            "Snare" => Some((&[1], "efffffeffffefffffbffff")),
            "Tom" => Some((&[1], "efffffffffeffffffffffff")),
            "TransientDesigner" => Some((&[0], "fffb")),
            _ => None,
        };
        let (_, members) = schema.filter(|(versions, _)| versions.contains(&module.version)).ok_or_else(|| invalid(format!("{} version {} has no verified parameter schema.", module.kind, module.version)))?;
        let mut id = module.object_id;
        members
            .chars()
            .map(|kind| {
                id += if kind == 'e' { wide } else { narrow };
                self.scalar(id, kind == 'f', start, end)
            })
            .collect()
    }

    /// The mixer members of a Sound: `level`, `pan`, `mute`, `muteAudio`, `aux1`, `aux2`,
    /// `chokeGroup`, `chokeMode`.
    pub fn channel(&self, sound: &Sound) -> Result<BTreeMap<&'static str, f64>> {
        let fields: [(&'static str, i64, bool); 8] = if self.modern() {
            [("level", 26, true), ("pan", 30, true), ("mute", 34, false), ("muteAudio", 38, false), ("aux1", 43, true), ("aux2", 53, true), ("chokeGroup", 77, false), ("chokeMode", 82, false)]
        } else {
            [("level", 21, true), ("pan", 24, true), ("mute", 27, false), ("muteAudio", 30, false), ("aux1", 34, true), ("aux2", 42, true), ("chokeGroup", 61, false), ("chokeMode", 65, false)]
        };
        let mut result = BTreeMap::new();
        for (name, delta, float) in fields {
            result.insert(name, self.scalar(sound.object_id + delta + self.sound_layout.channel_shift, float, sound.offset, sound.name_offset)?);
        }
        let valid = (0.0..=1.0).contains(&result["level"]) && (-1.0..=1.0).contains(&result["pan"]) && (0.0..=16.0).contains(&result["chokeGroup"]) && [0.0, 1.0].contains(&result["mute"]) && [0.0, 1.0].contains(&result["muteAudio"]);
        if !valid {
            return Err(invalid("Sound mixer controls are outside their supported range."));
        }
        Ok(result)
    }

    /// The sample zones of a Sampler Module.
    pub fn zones(&self, module: &Module) -> Result<Vec<Zone>> {
        let first = self.modules.iter().find(|candidate| candidate.kind == "Sampler").filter(|_| module.kind == "Sampler" && module.version <= 2).ok_or_else(|| invalid("unsupported sampler layout."))?;
        let view = self.view();
        let integer = |cursor: &mut usize, limit: usize| view.integer(cursor, limit).ok();
        // The Zone class is introduced by the first Sampler's first zone.
        let first_hits = view.matches(&encoded(first.object_id + 1), first.body_offset, first.end.min(first.body_offset + 48));
        let mut class_ids: Vec<u64> = Vec::new();
        for hit in first_hits.into_iter().filter(|hit| *hit >= 6) {
            let mut cursor = hit - 6;
            let class = integer(&mut cursor, hit);
            if class.is_some() && integer(&mut cursor, hit) == Some(1) && integer(&mut cursor, hit) == Some(2) && cursor == hit {
                class_ids.extend(class);
            }
        }
        let &[class_id] = class_ids.as_slice() else { return Err(invalid("the sample Zone class is ambiguous.")) };
        let class_bytes = encoded(class_id as i64);
        let end = module.end.min(self.bytes.len());
        let is_first = module.object_id == first.object_id;
        let skip_zero_pair = |cursor: &mut usize| {
            if *cursor + 2 <= end && self.bytes[*cursor] == 0 && self.bytes[*cursor + 1] == 0 {
                *cursor += 2;
            }
        };
        let mut candidates: Vec<(usize, i64, String, String, String)> = Vec::new();
        for hit in view.matches(&class_bytes, module.body_offset, end) {
            let mut cursor = hit + class_bytes.len();
            if is_first && cursor + 4 <= end && self.bytes[cursor..cursor + 4] == [1, 1, 1, 2] {
                cursor += 4;
            }
            let Some(id) = integer(&mut cursor, end).map(|id| id as i64).filter(|id| *id > module.object_id) else { continue };
            skip_zero_pair(&mut cursor);
            if integer(&mut cursor, end) != Some(2) {
                continue;
            }
            let Some(path) = view.string(&mut cursor, end, 4096).ok().filter(|path| !path.is_empty()) else { continue };
            skip_zero_pair(&mut cursor);
            let (Ok(origin), Ok(library)) = (view.string(&mut cursor, end, 4096), view.string(&mut cursor, end, 4096)) else { continue };
            candidates.push((hit, id, path, origin, library));
        }
        if candidates.is_empty() || candidates.len() > 128 || candidates[0].1 != module.object_id + 1 {
            return Err(invalid("the sampler has no safely decoded sample zones."));
        }
        // The vector's stored count must match the parsed Zone objects exactly.
        let vector = candidates[0].0;
        let mut count_pattern = encoded(candidates.len() as i64);
        count_pattern.extend([1, 1]);
        let gap = if is_first { 3 } else { 0 };
        if vector < count_pattern.len() + gap || self.bytes[vector - count_pattern.len() - gap..vector - gap] != count_pattern[..] {
            return Err(invalid("the sampler's declared zone count does not match its objects."));
        }
        const KEYS: [&str; 16] = ["startFrame", "endFrame", "loopStartFrame", "loopEndFrame", "loopEnabled", "crossfadeFrames", "unused1", "unused2", "root", "keyLow", "keyHigh", "velocityLow", "velocityHigh", "gain", "pan", "tune"];
        let deltas: [i64; 16] = if self.modern() { [4, 7, 11, 14, 18, 22, 26, 30, 34, 38, 41, 45, 48, 52, 56, 60] } else { [3, 5, 8, 10, 13, 16, 19, 22, 25, 28, 30, 33, 35, 38, 41, 44] };
        let mut zones = Vec::with_capacity(candidates.len());
        for (index, candidate) in candidates.iter().enumerate() {
            let zone_end = candidates.get(index + 1).map(|next| next.0).unwrap_or(end);
            let mut values: BTreeMap<&'static str, f64> = BTreeMap::new();
            for (position, key) in KEYS.iter().enumerate() {
                values.insert(key, self.scalar(candidate.1 + deltas[position], position >= 13, candidate.0, zone_end)?);
            }
            if ["root", "keyLow", "keyHigh", "velocityLow", "velocityHigh"].iter().any(|key| !(0.0..=127.0).contains(&values[key])) {
                return Err(invalid("invalid zone MIDI range."));
            }
            // Sampler version 0 stores an unset loop end as 0; it then spans the playback range.
            if module.version == 0 && values["loopEndFrame"] == 0.0 && values["loopStartFrame"] == 0.0 {
                values.insert("loopEndFrame", values["endFrame"]);
            }
            let valid = values["keyLow"] <= values["keyHigh"]
                && values["velocityLow"] <= values["velocityHigh"]
                && values["startFrame"] >= 0.0
                && values["endFrame"] >= values["startFrame"]
                && values["loopStartFrame"] >= 0.0
                && values["loopEndFrame"] >= values["loopStartFrame"]
                && (0.0..=1.0).contains(&values["gain"])
                && (-1.0..=1.0).contains(&values["pan"])
                && (-36.0..=36.0).contains(&values["tune"])
                && [0.0, 1.0].contains(&values["loopEnabled"]);
            if !valid {
                return Err(invalid("invalid zone sample range."));
            }
            zones.push(Zone { object_id: candidate.1, path: candidate.2.clone(), origin: candidate.3.clone(), library: candidate.4.clone(), values });
        }
        Ok(zones)
    }

    /// The single audio file an Audio module (Maschine 2's loop player) refers to. Its sample
    /// reference (record version 2, relative path, original location, library name) sits near
    /// the start of the module body. Only the reference is decoded, not the playback controls.
    pub fn audio_module_sample(&self, module: &Module) -> Result<Zone> {
        if module.kind != "AudioModule" {
            return Err(invalid("not an Audio module."));
        }
        const AUDIO_EXTENSIONS: [&str; 10] = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "rx2", "rex", "rcy"];
        let view = self.view();
        let limit = module.end.min(module.body_offset + 4096).min(self.bytes.len());
        let mut found: Vec<(String, String, String)> = Vec::new();
        for start in module.body_offset..limit.min(module.body_offset + 96) {
            let mut cursor = start;
            if view.integer(&mut cursor, limit).ok() != Some(2) {
                continue;
            }
            let Some(path) = view.string(&mut cursor, limit, 4096).ok().filter(|path| !path.is_empty() && AUDIO_EXTENSIONS.contains(&crate::util::extension(&path.replace('\\', "/")).as_str())) else { continue };
            if cursor + 2 <= limit && self.bytes[cursor] == 0 && self.bytes[cursor + 1] == 0 {
                cursor += 2;
            }
            let (Ok(origin), Ok(library)) = (view.string(&mut cursor, limit, 4096), view.string(&mut cursor, limit, 4096)) else { continue };
            found.push((path, origin, library));
        }
        let [(path, origin, library)] = found.as_slice() else { return Err(invalid("the Audio module's sample reference is missing or ambiguous.")) };
        // The reference covers the whole file (end markers of -1) at unity gain.
        let values: BTreeMap<&'static str, f64> = [
            ("keyLow", 0.0), ("keyHigh", 127.0), ("velocityLow", 0.0), ("velocityHigh", 127.0), ("root", 60.0), ("pan", 0.0), ("tune", 0.0), ("gain", UNITY_ZONE_GAIN),
            ("loopEnabled", 0.0), ("startFrame", 0.0), ("endFrame", -1.0), ("loopStartFrame", 0.0), ("loopEndFrame", -1.0),
        ]
        .into_iter()
        .collect();
        Ok(Zone { object_id: module.object_id, path: path.clone(), origin: origin.clone(), library: library.clone(), values })
    }

    /// The Sampler's own controls: flat fields named by their member offsets (`0xc8` ...) and two
    /// nine-control envelopes (`ampEnvelope.0` ... `filterEnvelope.8`). See
    /// [`super::mapping::map_sampler`] for what they mean.
    pub fn sampler_parameters(&self, module: &Module, zones: &[Zone]) -> Result<BTreeMap<String, f64>> {
        let last = zones.last().ok_or_else(|| invalid("empty sample zone vector."))?;
        let modern = self.modern();
        let (wide, narrow): (i64, i64) = if modern { (5, 4) } else { (4, 3) };
        let mut id = last.object_id + if modern { 79 } else { 58 };
        const FIELDS: [(&str, char); 27] = [
            ("0xc0", 'e'), ("0xc8", 'e'), ("0xd0", 'f'), ("0xd8", 'f'), ("0xe0", 'e'), ("0xe8", 'e'), ("0xf0", 'b'), ("0xf8", 'f'), ("0x100", 'f'), ("0x128", 'f'), ("0x130", 'f'), ("0x138", 'f'), ("0x140", 'f'),
            ("0x148", 'e'), ("0x150", 'f'), ("0x158", 'f'), ("0x160", 'f'), ("0x168", 'f'), ("0x170", 'f'), ("0x178", 'f'), ("0x180", 'f'), ("0x188", 'f'), ("0x190", 'f'), ("0x198", 'f'), ("0x1a0", 'f'), ("0x1a8", 'f'), ("0x1b0", 'f'),
        ];
        let mut result = BTreeMap::new();
        // Sampler version 0 (Maschine 1.x) has no member at 0xc0; every later field keeps its kind and order.
        for (key, kind) in FIELDS.iter().filter(|(key, _)| module.version > 0 || *key != "0xc0") {
            id += if *kind == 'e' { wide } else { narrow };
            result.insert(key.to_string(), self.scalar(id, *kind == 'f', module.body_offset, module.end)?);
        }
        for envelope in ["ampEnvelope", "filterEnvelope"] {
            for index in 0..9 {
                id += narrow;
                result.insert(format!("{envelope}.{index}"), self.scalar(id, true, module.body_offset, module.end)?);
            }
        }
        Ok(result)
    }
}

/// The gain value of a zone at unity level (0 dB) in the native 0..1 level scale.
pub const UNITY_ZONE_GAIN: f64 = 0.75;

/// A sample zone as stored: where the sample is and its native ranges (inclusive frames, MIDI
/// keys and velocities, native level).
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    /// The Zone's object ID.
    pub object_id: i64,
    /// The sample, relative to the expansion library.
    pub path: String,
    /// Where the sample was when the group was saved.
    pub origin: String,
    /// The library's name.
    pub library: String,
    /// `startFrame`, `endFrame`, `loopStartFrame`, `loopEndFrame`, `loopEnabled`, `root`,
    /// `keyLow`, `keyHigh`, `velocityLow`, `velocityHigh`, `gain`, `pan`, `tune` (and, for
    /// zones read from a Sampler, `crossfadeFrames`).
    pub values: BTreeMap<&'static str, f64>,
}

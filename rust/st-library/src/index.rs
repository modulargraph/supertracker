//! The in-memory index and its files.
//!
//! An [`Item`] is one indexed file. Items are immutable and shared (`Arc`): a search works on
//! a snapshot of the item list while scans and edits replace individual items. Repeated
//! strings (roots, packs, categories, keys, tags ...) are interned in [`Tables`]; items hold
//! the slot numbers, which also makes facet counting integer work.
//!
//! Files (in the support directory the caller gives):
//!
//! | File | Content |
//! |---|---|
//! | `roots.json` | the library folders and the folders the user removed (user settings) |
//! | `index.bin` | the index cache: items, scan reports, verified expansion folders |
//! | `metadata.json` | the user's labels per file (authoritative, never rebuilt) |
//! | `favorites.json` | favorite paths |
//!
//! `index.bin` is a cache in a compact binary layout with a checksum. A file of another
//! version, a truncated or a damaged one is ignored and the index is rebuilt by scanning.

use crate::api::{EditableMetadata, RootReport};
use crate::error::{Error, Result};
use crate::infer::{base_collection, canonical_key, collection_root, infer_sample_type, key_slot};
use crate::util::{extension, file_name, fold, is_inside};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------------------------
// String tables

/// Which table a string is interned in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Field {
    Category = 0,
    Root = 1,
    Pack = 2,
    Key = 3,
    SampleType = 4,
    Extension = 5,
    Tag = 6,
    Source = 7,
    CollectionRoot = 8,
}
pub(crate) const FIELD_COUNT: usize = 9;

/// An append-only string table: values, and their folded form for filters.
#[derive(Default, Clone)]
pub(crate) struct Table {
    ids: HashMap<Arc<str>, u32>,
    pub values: Vec<Arc<str>>,
    pub folded: Vec<Arc<str>>,
}

/// The string tables of a library, shared by every item of it.
#[derive(Default)]
pub(crate) struct Tables {
    tables: [Mutex<Table>; FIELD_COUNT],
}

impl Tables {
    pub fn id(&self, field: Field, value: &str) -> u32 {
        let mut table = crate::util::lock(&self.tables[field as usize]);
        if let Some(id) = table.ids.get(value) {
            return *id;
        }
        let id = table.values.len() as u32;
        let shared: Arc<str> = Arc::from(value);
        // Keys compare by slot so enharmonic spellings are one value.
        let folded: Arc<str> = if field == Field::Key && value != "unknown" { Arc::from(key_slot(value)) } else { Arc::from(fold(value)) };
        table.ids.insert(shared.clone(), id);
        table.values.push(shared);
        table.folded.push(folded);
        id
    }

    pub fn value(&self, field: Field, id: u32) -> Arc<str> {
        crate::util::lock(&self.tables[field as usize]).values.get(id as usize).cloned().unwrap_or_else(|| Arc::from(""))
    }

    pub fn snapshot(&self, field: Field) -> (Vec<Arc<str>>, Vec<Arc<str>>) {
        let table = crate::util::lock(&self.tables[field as usize]);
        (table.values.clone(), table.folded.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// Items

/// What kind of file an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Sample,
    Nks,
    Kit,
}

impl Kind {
    pub fn text(self) -> &'static str {
        match self {
            Kind::Sample => "sample",
            Kind::Nks => "nks",
            Kind::Kit => "kit",
        }
    }
    pub fn of_extension(extension: &str) -> Option<Kind> {
        match extension {
            "wav" | "wave" | "aif" | "aiff" | "flac" | "mp3" | "ogg" | "m4a" | "caf" => Some(Kind::Sample),
            "nksf" | "nksfx" => Some(Kind::Nks),
            "stkit" | "mxgrp" => Some(Kind::Kit),
            _ => None,
        }
    }
}

/// An indexed file as plain data: what a scan produces and what the cache stores.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Record {
    pub path: String,
    pub root: String,
    pub name: String,
    pub kind: Kind,
    pub category: String,
    pub tags: Vec<String>,
    pub pack: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub size: u64,
    pub modified: f64,
    pub metadata_source: String,
    pub preview_path: Option<String>,
    pub duration: Option<f64>,
    pub duration_checked: bool,
    pub kit_sound_count: Option<u32>,
    pub kit_metadata_error: Option<String>,
}

/// The user's labels for one file. Absent fields follow the inferred value.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Override {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Zero is an explicit "unknown"; absent restores inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    /// Empty is an explicit "unknown".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_type: Option<String>,
}

impl Override {
    pub fn is_empty(&self) -> bool {
        self.category.is_none() && self.tags.is_none() && self.bpm.is_none() && self.key.is_none() && self.sample_type.is_none()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| Value::Object(Map::new()))
    }

    /// Applies edits: a value sets the field, `null` restores the inferred value.
    pub fn merge(&mut self, changes: &Map<String, Value>) -> Result<()> {
        if changes.keys().any(|key| !["category", "tags", "bpm", "key", "sampleType"].contains(&key.as_str())) {
            return Err(Error::invalid("Unknown library metadata field."));
        }
        for (field, value) in changes {
            let reset = value.is_null();
            match field.as_str() {
                "category" => {
                    if reset {
                        self.category = None;
                        continue;
                    }
                    let text = value.as_str().map(str::trim).filter(|text| !text.is_empty() && value.as_str().is_some_and(|raw| raw.chars().count() <= 80));
                    self.category = Some(text.ok_or_else(|| Error::invalid("Choose a category up to 80 characters, or restore its inferred value."))?.to_string());
                }
                "tags" => {
                    if reset {
                        self.tags = None;
                        continue;
                    }
                    let values: Option<Vec<&str>> = value.as_array().filter(|values| values.len() <= 64).and_then(|values| values.iter().map(Value::as_str).collect());
                    let values = values.ok_or_else(|| Error::invalid("Use up to 64 tags."))?;
                    let mut found = std::collections::HashSet::new();
                    let mut cleaned = Vec::new();
                    for value in values {
                        let text = value.trim();
                        if text.chars().count() > 80 {
                            return Err(Error::invalid("Each tag can contain up to 80 characters."));
                        }
                        if !text.is_empty() && found.insert(fold(text)) {
                            cleaned.push(text.to_string());
                        }
                    }
                    self.tags = Some(cleaned);
                }
                "bpm" => {
                    if reset {
                        self.bpm = None;
                        continue;
                    }
                    let number = value.as_f64().filter(|number| number.is_finite() && (*number == 0.0 || (1.0..=999.0).contains(number)));
                    self.bpm = Some(number.ok_or_else(|| Error::invalid("Tempo must be 1–999 BPM, or blank for unknown."))?);
                }
                "key" => {
                    if reset {
                        self.key = None;
                        continue;
                    }
                    let text = value.as_str().filter(|text| text.chars().count() <= 24).ok_or_else(|| Error::invalid("Choose a musical key, or leave it blank for unknown."))?;
                    let normalized = text.trim().replace('♯', "#").replace('♭', "b");
                    if normalized.is_empty() {
                        self.key = Some(String::new());
                        continue;
                    }
                    // The user's spelling is kept ("Bb minor"); only its form is normalized.
                    if canonical_key(&normalized).is_none() {
                        return Err(Error::invalid("Use a key such as C, F# minor or Bb major."));
                    }
                    let mut characters = normalized.chars();
                    let letter = characters.next().map(|letter| letter.to_ascii_uppercase()).unwrap_or('C');
                    let rest: String = characters.collect();
                    let (accidental, mode) = match rest.chars().next() {
                        Some('#') => ("#", &rest[1..]),
                        Some('b' | 'B') if canonical_key(&format!("{letter}{}", &rest[1..])).is_some() => ("b", &rest[1..]),
                        _ => ("", rest.as_str()),
                    };
                    let mode = match mode.trim().to_lowercase().as_str() {
                        "m" | "min" | "minor" => " minor",
                        "maj" | "major" => " major",
                        _ => "",
                    };
                    self.key = Some(format!("{letter}{accidental}{mode}"));
                }
                "sampleType" => {
                    if reset {
                        self.sample_type = None;
                        continue;
                    }
                    let text = value.as_str().filter(|text| ["oneshot", "loop", "unknown"].contains(text));
                    self.sample_type = Some(text.ok_or_else(|| Error::invalid("Sound type must be one-shot, loop or unknown."))?.to_string());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// One indexed file, ready for searching. Immutable; replaced as a whole.
pub(crate) struct Item {
    pub path: Box<str>,
    pub name: Box<str>,
    pub kind: Kind,
    pub category: u32,
    pub root: u32,
    pub pack: u32,
    /// Slot of the key, or of "unknown".
    pub key: u32,
    pub has_key: bool,
    pub sample_type: u32,
    pub extension: u32,
    pub tags: Box<[u32]>,
    pub source: u32,
    pub bpm: Option<f64>,
    pub size: u64,
    pub modified: f64,
    pub preview: Option<Box<str>>,
    pub duration: Option<f64>,
    pub duration_checked: bool,
    pub kit_sound_count: Option<u32>,
    pub kit_error: Option<Box<str>>,
    /// Index into [`crate::infer::COLLECTION_NAMES`] before expansions are verified.
    pub base_collection: u8,
    pub collection_root: Option<u32>,
    /// Folded name, a line break, folded path: what free-text words are matched against.
    pub folded: Box<[u8]>,
    pub name_length: u32,
    /// The item without the user's labels, when it has any.
    pub inferred: Option<Arc<Item>>,
    pub user: Option<Box<Override>>,
}

impl Item {
    pub fn new(record: &Record, tables: &Tables) -> Item {
        Item::build(record, tables, None)
    }

    fn build(record: &Record, tables: &Tables, sample_type: Option<&str>) -> Item {
        let folded_name = fold(&record.name);
        let mut folded = Vec::with_capacity(folded_name.len() + record.path.len() + 1);
        folded.extend_from_slice(folded_name.as_bytes());
        folded.push(b'\n');
        folded.extend_from_slice(fold(&record.path).as_bytes());
        let base = base_collection(&record.path);
        let inferred_type = infer_sample_type(&record.name, &record.path, &record.root, record.kind == Kind::Sample, record.bpm.is_some());
        Item {
            path: record.path.as_str().into(),
            name: record.name.as_str().into(),
            kind: record.kind,
            category: tables.id(Field::Category, &record.category),
            root: tables.id(Field::Root, &record.root),
            pack: tables.id(Field::Pack, &record.pack),
            key: tables.id(Field::Key, record.key.as_deref().unwrap_or("unknown")),
            has_key: record.key.is_some(),
            sample_type: tables.id(Field::SampleType, sample_type.unwrap_or(inferred_type)),
            extension: tables.id(Field::Extension, &extension(&record.path)),
            tags: record.tags.iter().map(|tag| tables.id(Field::Tag, tag)).collect(),
            source: tables.id(Field::Source, &record.metadata_source),
            bpm: record.bpm,
            size: record.size,
            modified: record.modified,
            preview: record.preview_path.as_deref().map(Into::into),
            duration: record.duration,
            duration_checked: record.duration_checked,
            kit_sound_count: record.kit_sound_count,
            kit_error: record.kit_metadata_error.as_deref().map(Into::into),
            base_collection: base as u8,
            collection_root: collection_root(&record.path, &record.root, base).map(|root| tables.id(Field::CollectionRoot, &root)),
            name_length: folded_name.len() as u32,
            folded: folded.into_boxed_slice(),
            inferred: None,
            user: None,
        }
    }

    /// The item as plain data (with the user's labels applied, if it has any).
    pub fn record(&self, tables: &Tables) -> Record {
        Record {
            path: self.path.to_string(),
            root: tables.value(Field::Root, self.root).to_string(),
            name: self.name.to_string(),
            kind: self.kind,
            category: tables.value(Field::Category, self.category).to_string(),
            tags: self.tags.iter().map(|tag| tables.value(Field::Tag, *tag).to_string()).collect(),
            pack: tables.value(Field::Pack, self.pack).to_string(),
            bpm: self.bpm,
            key: self.has_key.then(|| tables.value(Field::Key, self.key).to_string()),
            size: self.size,
            modified: self.modified,
            metadata_source: tables.value(Field::Source, self.source).to_string(),
            preview_path: self.preview.as_deref().map(String::from),
            duration: self.duration,
            duration_checked: self.duration_checked,
            kit_sound_count: self.kit_sound_count,
            kit_metadata_error: self.kit_error.as_deref().map(String::from),
        }
    }

    /// The item without the user's labels.
    pub fn base(self: &Arc<Item>) -> Arc<Item> {
        self.inferred.clone().unwrap_or_else(|| self.clone())
    }

    /// The item with `labels` applied on top of its inferred values (or without labels).
    pub fn applying(self: &Arc<Item>, labels: Option<&Override>, tables: &Tables) -> Arc<Item> {
        let base = self.base();
        let Some(labels) = labels.filter(|labels| !labels.is_empty()) else { return base };
        let mut record = base.record(tables);
        if let Some(category) = &labels.category {
            record.category = category.clone();
        }
        if let Some(tags) = &labels.tags {
            record.tags = tags.clone();
        }
        // Zero and empty are explicit unknowns, distinct from absent labels.
        match labels.bpm {
            Some(bpm) if bpm == 0.0 => record.bpm = None,
            Some(bpm) => record.bpm = Some(bpm),
            None => {}
        }
        match labels.key.as_deref() {
            Some("") => record.key = None,
            Some(key) => record.key = Some(key.to_string()),
            None => {}
        }
        // The inferred type looks at the inferred tempo, not at the user's.
        let base_type = tables.value(Field::SampleType, base.sample_type);
        let mut item = Item::build(&record, tables, Some(labels.sample_type.as_deref().unwrap_or(&base_type)));
        item.inferred = Some(base);
        item.user = Some(Box::new(labels.clone()));
        Arc::new(item)
    }

    /// The item with a (possibly unknown) header length recorded, labels kept.
    pub fn replacing_duration(self: &Arc<Item>, duration: Option<f64>, tables: &Tables) -> Arc<Item> {
        let mut record = self.base().record(tables);
        record.duration = duration;
        record.duration_checked = true;
        Arc::new(Item::new(&record, tables)).applying(self.user.as_deref(), tables)
    }

    pub fn folded_name(&self) -> &[u8] {
        &self.folded[..self.name_length as usize]
    }

    pub fn editable(&self, tables: &Tables) -> EditableMetadata {
        EditableMetadata {
            category: tables.value(Field::Category, self.category).to_string(),
            tags: self.tags.iter().map(|tag| tables.value(Field::Tag, *tag).to_string()).collect(),
            bpm: self.bpm.unwrap_or(0.0),
            key: if self.has_key { tables.value(Field::Key, self.key).to_string() } else { String::new() },
            sample_type: tables.value(Field::SampleType, self.sample_type).to_string(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Roots

/// A library folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Root {
    pub path: String,
    pub name: String,
    pub source: String,
}

impl Root {
    pub fn new(path: &str, source: &str) -> Root {
        let path = crate::util::normalized(path);
        Root { name: file_name(&path).to_string(), path, source: source.to_string() }
    }
}

#[derive(Serialize, Deserialize)]
struct RootsFile {
    version: u32,
    roots: Vec<Root>,
    #[serde(default)]
    excluded: Vec<String>,
}

pub(crate) fn load_roots(folder: &Path) -> Option<(Vec<Root>, Vec<String>)> {
    let data = crate::util::read_bounded(&folder.join("roots.json"), 4 * 1024 * 1024)?;
    let file: RootsFile = serde_json::from_slice(&data).ok()?;
    (file.version == 1).then_some((file.roots, file.excluded))
}

pub(crate) fn save_roots(folder: &Path, roots: &[Root], excluded: &[String]) -> Result<()> {
    let file = RootsFile { version: 1, roots: roots.to_vec(), excluded: excluded.to_vec() };
    crate::util::write_atomic(&folder.join("roots.json"), &serde_json::to_vec_pretty(&file).unwrap_or_default())
}

// ---------------------------------------------------------------------------------------------
// The index cache

const MAGIC: &[u8; 8] = b"STLIBIDX";
/// Bumped whenever the layout or the inference rules change: an older cache is rebuilt.
pub(crate) const CACHE_VERSION: u32 = 1;

/// What the cache file holds.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Cache {
    pub items: Vec<Record>,
    pub errors: Vec<String>,
    pub reports: Vec<RootReport>,
    pub verified_expansion_roots: Vec<String>,
}

fn fnv64(bytes: &[u8]) -> u64 {
    // Eight bytes per step: this is a checksum against truncation and damage, not a hash table key.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        hash = (hash ^ u64::from_le_bytes(chunk.try_into().unwrap_or([0; 8]))).wrapping_mul(0x0000_0100_0000_01b3);
    }
    for byte in chunks.remainder() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    strings: HashMap<String, u32>,
    table: Vec<String>,
}

impl Writer {
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn f64(&mut self, value: Option<f64>) {
        self.bytes.extend_from_slice(&value.unwrap_or(f64::NAN).to_le_bytes());
    }
    fn text(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.bytes.extend_from_slice(value.as_bytes());
    }
    fn shared(&mut self, value: &str) {
        let id = match self.strings.get(value) {
            Some(id) => *id,
            None => {
                let id = self.table.len() as u32;
                self.strings.insert(value.to_string(), id);
                self.table.push(value.to_string());
                id
            }
        };
        self.u32(id);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    table: Vec<String>,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.position..self.position.checked_add(count)?)?;
        self.position += count;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes)
    }
    fn f64(&mut self) -> Option<Option<f64>> {
        let value = f64::from_le_bytes(self.take(8)?.try_into().ok()?);
        Some((!value.is_nan()).then_some(value))
    }
    fn text(&mut self) -> Option<String> {
        let length = self.u32()? as usize;
        String::from_utf8(self.take(length)?.to_vec()).ok()
    }
    fn shared(&mut self) -> Option<String> {
        let id = self.u32()? as usize;
        self.table.get(id).cloned()
    }
    fn count(&mut self, limit: usize) -> Option<usize> {
        let count = self.u32()? as usize;
        (count <= limit && count <= self.bytes.len()).then_some(count)
    }
}

/// The cache as bytes: header, checksum, string table, then the sections.
pub(crate) fn encode_cache(cache: &Cache) -> Vec<u8> {
    let mut body = Writer::default();
    body.u32(cache.items.len() as u32);
    for item in &cache.items {
        // Paths are stored relative to their root; the root is a shared string.
        body.shared(&item.root);
        let relative = if is_inside(&item.path, &item.root) { Some(&item.path[item.root.len()..]) } else { None };
        body.u8(u8::from(relative.is_some()));
        body.text(relative.unwrap_or(&item.path));
        let derived = crate::util::stem(&item.path) == item.name;
        let flags = u8::from(item.duration_checked) | u8::from(derived) << 1 | u8::from(item.preview_path.is_some()) << 2 | u8::from(item.kit_sound_count.is_some()) << 3 | u8::from(item.kit_metadata_error.is_some()) << 4 | u8::from(item.key.is_some()) << 5;
        body.u8(flags);
        body.u8(item.kind as u8);
        if !derived {
            body.text(&item.name);
        }
        body.shared(&item.category);
        body.shared(&item.pack);
        if let Some(key) = &item.key {
            body.shared(key);
        }
        body.shared(&item.metadata_source);
        body.f64(item.bpm);
        body.u64(item.size);
        body.f64(Some(item.modified));
        body.f64(item.duration);
        if let Some(preview) = &item.preview_path {
            body.text(preview);
        }
        if let Some(count) = item.kit_sound_count {
            body.u32(count);
        }
        if let Some(error) = &item.kit_metadata_error {
            body.text(error);
        }
        body.u32(item.tags.len() as u32);
        for tag in &item.tags {
            body.shared(tag);
        }
    }
    body.u32(cache.errors.len() as u32);
    for error in &cache.errors {
        body.text(error);
    }
    body.u32(cache.reports.len() as u32);
    for report in &cache.reports {
        body.text(&report.path);
        body.text(&report.status);
        body.u64(report.visited);
        body.u64(report.matched);
        body.u64(report.skipped);
        body.u32(report.errors.len() as u32);
        for error in &report.errors {
            body.text(error);
        }
    }
    body.u32(cache.verified_expansion_roots.len() as u32);
    for root in &cache.verified_expansion_roots {
        body.text(root);
    }

    let mut payload = Writer::default();
    payload.u32(body.table.len() as u32);
    for value in &body.table {
        payload.text(value);
    }
    payload.bytes.extend_from_slice(&body.bytes);
    let mut file = Vec::with_capacity(payload.bytes.len() + 28);
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    file.extend_from_slice(&(payload.bytes.len() as u64).to_le_bytes());
    file.extend_from_slice(&fnv64(&payload.bytes).to_le_bytes());
    file.extend_from_slice(&payload.bytes);
    file
}

/// Reads cache bytes. `None` for another version, a truncated or a damaged file.
pub(crate) fn decode_cache(bytes: &[u8]) -> Option<Cache> {
    if bytes.len() < 28 || &bytes[0..8] != MAGIC || u32::from_le_bytes(bytes[8..12].try_into().ok()?) != CACHE_VERSION {
        return None;
    }
    let length = u64::from_le_bytes(bytes[12..20].try_into().ok()?) as usize;
    let checksum = u64::from_le_bytes(bytes[20..28].try_into().ok()?);
    let payload = bytes.get(28..)?;
    if payload.len() != length || fnv64(payload) != checksum {
        return None;
    }
    let mut reader = Reader { bytes: payload, position: 0, table: Vec::new() };
    let strings = reader.count(usize::MAX)?;
    reader.table = (0..strings).map(|_| reader.text()).collect::<Option<Vec<_>>>()?;
    let mut cache = Cache::default();
    let items = reader.count(usize::MAX)?;
    cache.items.reserve(items);
    for _ in 0..items {
        let root = reader.shared()?;
        let relative = reader.u8()? != 0;
        let rest = reader.text()?;
        let path = if relative { format!("{root}{rest}") } else { rest };
        let flags = reader.u8()?;
        let kind = match reader.u8()? {
            0 => Kind::Sample,
            1 => Kind::Nks,
            2 => Kind::Kit,
            _ => return None,
        };
        let name = if flags & 2 != 0 { crate::util::stem(&path).to_string() } else { reader.text()? };
        let category = reader.shared()?;
        let pack = reader.shared()?;
        let key = if flags & 32 != 0 { Some(reader.shared()?) } else { None };
        let metadata_source = reader.shared()?;
        let bpm = reader.f64()?;
        let size = reader.u64()?;
        let modified = reader.f64()?.unwrap_or(0.0);
        let duration = reader.f64()?;
        let preview_path = if flags & 4 != 0 { Some(reader.text()?) } else { None };
        let kit_sound_count = if flags & 8 != 0 { Some(reader.u32()?) } else { None };
        let kit_metadata_error = if flags & 16 != 0 { Some(reader.text()?) } else { None };
        let tag_count = reader.count(4096)?;
        let tags = (0..tag_count).map(|_| reader.shared()).collect::<Option<Vec<_>>>()?;
        cache.items.push(Record { path, root, name, kind, category, tags, pack, bpm, key, size, modified, metadata_source, preview_path, duration, duration_checked: flags & 1 != 0, kit_sound_count, kit_metadata_error });
    }
    let errors = reader.count(100_000)?;
    cache.errors = (0..errors).map(|_| reader.text()).collect::<Option<Vec<_>>>()?;
    let reports = reader.count(100_000)?;
    for _ in 0..reports {
        let (path, status, visited, matched, skipped) = (reader.text()?, reader.text()?, reader.u64()?, reader.u64()?, reader.u64()?);
        let count = reader.count(1000)?;
        let errors = (0..count).map(|_| reader.text()).collect::<Option<Vec<_>>>()?;
        cache.reports.push(RootReport { path, status, visited, matched, skipped, errors });
    }
    let roots = reader.count(10_000_000)?;
    cache.verified_expansion_roots = (0..roots).map(|_| reader.text()).collect::<Option<Vec<_>>>()?;
    (reader.position == payload.len()).then_some(cache)
}

/// The user's labels file.
#[derive(Serialize, Deserialize)]
pub(crate) struct MetadataFile {
    pub version: u32,
    pub items: std::collections::BTreeMap<String, Override>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(path: &str, name: &str) -> Record {
        Record {
            path: path.to_string(),
            root: "/lib".to_string(),
            name: name.to_string(),
            kind: Kind::Sample,
            category: "Kick".to_string(),
            tags: vec!["Kick".to_string(), "Loop".to_string()],
            pack: "Pack".to_string(),
            bpm: Some(120.0),
            key: Some("C minor".to_string()),
            size: 1024,
            modified: 1_700_000_000.25,
            metadata_source: "filename and folders".to_string(),
            preview_path: None,
            duration: Some(0.5),
            duration_checked: true,
            kit_sound_count: None,
            kit_metadata_error: None,
        }
    }

    #[test]
    fn cache_round_trips_and_rejects_damage() {
        let mut preset = record("/elsewhere/Preset.nksf", "A Preset Name");
        preset.kind = Kind::Nks;
        preset.key = None;
        preset.bpm = None;
        preset.duration = None;
        preset.duration_checked = false;
        preset.preview_path = Some("/elsewhere/.previews/Preset.nksf.ogg".to_string());
        let mut kit = record("/lib/Kits/My.stkit", "My");
        kit.kind = Kind::Kit;
        kit.kit_sound_count = Some(7);
        kit.kit_metadata_error = Some("broken".to_string());
        let cache = Cache {
            items: vec![record("/lib/Pack/Kick_120.wav", "Kick_120"), preset, kit],
            errors: vec!["Library is offline".to_string()],
            reports: vec![RootReport { path: "/lib".to_string(), status: "complete".to_string(), visited: 9, matched: 3, skipped: 1, errors: vec!["x".to_string()] }],
            verified_expansion_roots: vec!["/lib/Pack".to_string()],
        };
        let bytes = encode_cache(&cache);
        assert_eq!(decode_cache(&bytes), Some(cache));
        assert_eq!(decode_cache(&bytes[..bytes.len() - 1]), None, "truncated");
        let mut damaged = bytes.clone();
        let middle = damaged.len() / 2;
        damaged[middle] ^= 0x40;
        assert_eq!(decode_cache(&damaged), None, "damaged");
        let mut other = bytes.clone();
        other[8] = 99;
        assert_eq!(decode_cache(&other), None, "another version");
        assert_eq!(decode_cache(b"{\"version\":1}"), None, "a file of another format");
    }

    #[test]
    fn labels_apply_over_inferred_values() {
        let tables = Tables::default();
        let item = Arc::new(Item::new(&record("/lib/Pack/Kick_120.wav", "Kick_120"), &tables));
        assert_eq!(&*tables.value(Field::SampleType, item.sample_type), "loop", "a tempo in the file name means a loop");
        let mut labels = Override::default();
        labels.merge(json!({ "category": " Snare ", "tags": ["One", "one", " Two "], "bpm": 0, "key": "", "sampleType": "oneshot" }).as_object().unwrap()).unwrap();
        let edited = item.applying(Some(&labels), &tables);
        let record = edited.record(&tables);
        assert_eq!((record.category.as_str(), record.tags.clone(), record.bpm, record.key.clone()), ("Snare", vec!["One".to_string(), "Two".to_string()], None, None));
        assert_eq!(&*tables.value(Field::SampleType, edited.sample_type), "oneshot");
        assert_eq!(edited.base().record(&tables).category, "Kick", "the inferred values are kept");
        // Restoring every field gives the inferred item back.
        labels.merge(json!({ "category": null, "tags": null, "bpm": null, "key": null, "sampleType": null }).as_object().unwrap()).unwrap();
        assert!(labels.is_empty());
        assert!(Arc::ptr_eq(&edited.applying(Some(&labels), &tables), &item));
        // A length keeps the labels.
        let mut only_key = Override::default();
        only_key.merge(json!({ "key": "bb MIN" }).as_object().unwrap()).unwrap();
        assert_eq!(only_key.key.as_deref(), Some("Bb minor"));
        let timed = item.applying(Some(&only_key), &tables).replacing_duration(Some(2.0), &tables);
        assert_eq!((timed.duration, timed.record(&tables).key.as_deref()), (Some(2.0), Some("Bb minor")));
        assert_eq!(&*tables.snapshot(Field::Key).1[timed.key as usize], "a#minor", "keys compare enharmonically");
    }

    #[test]
    fn label_edits_are_validated() {
        let rejects = |changes: Value| Override::default().merge(changes.as_object().unwrap()).is_err();
        assert!(rejects(json!({ "unknown": 1 })));
        assert!(rejects(json!({ "category": "" })) && rejects(json!({ "category": "x".repeat(81) })) && rejects(json!({ "category": 4 })));
        assert!(rejects(json!({ "tags": "one" })) && rejects(json!({ "tags": vec!["t"; 65] })) && rejects(json!({ "tags": ["x".repeat(81)] })) && rejects(json!({ "tags": [1] })));
        assert!(rejects(json!({ "bpm": 0.5 })) && rejects(json!({ "bpm": 1000 })) && rejects(json!({ "bpm": true })) && rejects(json!({ "bpm": "120" })));
        assert!(rejects(json!({ "key": "H" })) && rejects(json!({ "key": "C dorian" })) && rejects(json!({ "key": 3 })));
        assert!(rejects(json!({ "sampleType": "shot" })));
        let mut labels = Override::default();
        labels.merge(json!({ "bpm": 174.5, "key": "f#", "sampleType": "loop" }).as_object().unwrap()).unwrap();
        assert_eq!((labels.bpm, labels.key.as_deref(), labels.sample_type.as_deref()), (Some(174.5), Some("F#"), Some("loop")));
        assert_eq!(labels.to_value(), json!({ "bpm": 174.5, "key": "F#", "sampleType": "loop" }));
    }
}

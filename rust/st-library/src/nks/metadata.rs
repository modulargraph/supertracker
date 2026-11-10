//! Reading the metadata of an NKS preset file (`.nksf`, `.nksfx`).
//!
//! An NKS preset is a RIFF container (form `NIKS`) of chunks: `NISI` (summary: name, vendor,
//! bank chain, types, characters), `NICA` (controller pages), `PLID` (plug-in identity) and
//! `PCHK` (the plug-in's state). The first three hold a version word followed by one
//! MessagePack map. Reading metadata never reads `PCHK`: a vendor state can be hundreds of
//! megabytes and is skipped by seeking.

use super::msgpack::Decoder;
use crate::error::{Error, Result};
use crate::util::{file_name, is_readable_file, stem};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(crate) const METADATA_LIMIT: usize = 8 * 1024 * 1024;

const LIMITATIONS: [&str; 3] = [
    "Presets that identify an installed VST3 plug-in, or a VST2 plug-in whose vendor ships a compatible VST3, load directly with their mapped NKS control pages.",
    "NKS plug-in state is not an Audio Unit preset and is never applied to an Audio Unit.",
    "Komplete Kontrol hardware integration and Kontakt (.nki) instruments are not part of this host.",
];

/// One slot of an NKS controller page as the preset describes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetadataPage {
    /// Page number, from 0.
    pub index: usize,
    /// The page's section name, or "Page N".
    pub name: String,
    /// The slots: the preset's own fields plus `slot`, `name` and `requiresPluginParameterMapping`.
    pub parameters: Vec<Value>,
}

/// The browsable metadata of an NKS preset (the result of `library.nks`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NksMetadata {
    /// Preset name (the file name when the summary has none).
    pub name: String,
    /// The preset file.
    pub path: String,
    /// Always `NKS`.
    pub format: String,
    /// Type paths joined as "Type / Sub-type".
    pub categories: Vec<String>,
    /// Modes, tags and characters.
    pub tags: Vec<String>,
    /// The bank chain without empty positions.
    pub banks: Vec<String>,
    /// Type paths, at most two levels each.
    pub type_paths: Vec<Vec<String>>,
    /// Characters and modes.
    pub characters: Vec<String>,
    /// Whether the file carries plug-in state (`PCHK`).
    pub has_plugin_state: bool,
    /// What could not be read, in words.
    pub warnings: Vec<String>,
    /// What this host does not do with NKS presets.
    pub limitations: Vec<String>,
    /// Always false.
    #[serde(rename = "directAUPresetLoadingSupported")]
    pub direct_au_preset_loading_supported: bool,
    /// The chunk names in file order.
    pub chunks: Vec<String>,
    /// The version word of each metadata chunk.
    pub chunk_versions: BTreeMap<String, u32>,
    /// The `NISI` map as read.
    pub raw_metadata: Value,
    /// Author.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Vendor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// `INST` or `FX`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    /// The preset's UUID.
    #[serde(rename = "UUID", skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// First position of the bank chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// Second position of the bank chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bank: Option<String>,
    /// Third position of the bank chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_bank: Option<String>,
    /// The `PLID` map as read.
    #[serde(rename = "pluginID", skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<Value>,
    /// The `NICA` map as read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_mappings: Option<Value>,
    /// The controller pages of `NICA`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_pages: Option<Vec<MetadataPage>>,
    /// An audio preview next to the preset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_path: Option<String>,
}

pub(crate) fn read_exact_at(file: &mut File, offset: u64, count: usize, what: &str) -> Result<Vec<u8>> {
    if count > METADATA_LIMIT {
        return Err(Error::invalid("NKS read exceeds the metadata limit."));
    }
    let mut buffer = vec![0u8; count];
    let read = file.seek(SeekFrom::Start(offset)).and_then(|_| file.read_exact(&mut buffer));
    read.map_err(|_| Error::invalid(what))?;
    Ok(buffer)
}

pub(crate) fn little32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

pub(crate) fn tag(bytes: &[u8]) -> String {
    if bytes.iter().all(|byte| byte.is_ascii() && !byte.is_ascii_control()) {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        "????".to_string()
    }
}

/// A trimmed, non-empty string value.
pub(crate) fn nonempty(value: Option<&Value>) -> Option<String> {
    let trimmed = value?.as_str()?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Every non-empty string in a string or (nested) array of strings.
pub(crate) fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(_)) => nonempty(value).into_iter().collect(),
        Some(Value::Array(items)) => items.iter().flat_map(|item| strings(Some(item))).collect(),
        _ => Vec::new(),
    }
}

/// `values` without case-insensitive repeats, first spelling kept.
pub(crate) fn unique(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values.into_iter().filter(|value| seen.insert(value.to_lowercase())).collect()
}

fn category_paths(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => unique(
            items
                .iter()
                .filter_map(|item| {
                    let parts = strings(Some(item));
                    (!parts.is_empty()).then(|| parts.join(" / "))
                })
                .collect(),
        ),
        other => strings(other),
    }
}

/// Type paths are `[[Type], [Type, Sub-type]]`; each entry keeps at most its first two levels.
pub(crate) fn type_paths(value: Option<&Value>) -> Vec<Vec<String>> {
    match value {
        Some(Value::Array(items)) => {
            let mut seen = std::collections::HashSet::new();
            items
                .iter()
                .filter_map(|item| {
                    let parts: Vec<String> = strings(Some(item)).into_iter().take(2).collect();
                    (!parts.is_empty() && seen.insert(parts.join("\u{1f}").to_lowercase())).then_some(parts)
                })
                .collect()
        }
        other => strings(other).into_iter().map(|value| vec![value]).collect(),
    }
}

/// The bank chain by position (product, bank, sub-bank); empty positions stay empty.
pub(crate) fn bank_chain(info: &Map<String, Value>) -> Vec<String> {
    match info.get("bankchain") {
        Some(Value::Array(items)) => items.iter().map(|item| nonempty(Some(item)).unwrap_or_default()).collect(),
        _ => Vec::new(),
    }
}

fn controller_pages(mappings: &Map<String, Value>) -> Vec<MetadataPage> {
    let Some(Value::Array(pages)) = mappings.get("ni8") else { return Vec::new() };
    if !pages.iter().all(Value::is_array) {
        return Vec::new();
    }
    pages
        .iter()
        .enumerate()
        .map(|(index, slots)| {
            let mut section: Option<String> = None;
            let parameters = slots
                .as_array()
                .map(|slots| {
                    slots
                        .iter()
                        .enumerate()
                        .filter_map(|(slot, item)| {
                            let item = item.as_object()?;
                            if section.is_none() {
                                section = nonempty(item.get("section"));
                            }
                            let mut parameter = item.clone();
                            parameter.insert("slot".to_string(), Value::from(slot));
                            parameter.insert("name".to_string(), Value::String(nonempty(item.get("name")).unwrap_or_else(|| format!("Parameter {}", slot + 1))));
                            parameter.insert("requiresPluginParameterMapping".to_string(), Value::Bool(true));
                            Some(Value::Object(parameter))
                        })
                        .collect()
                })
                .unwrap_or_default();
            MetadataPage { index, name: section.unwrap_or_else(|| format!("Page {}", index + 1)), parameters }
        })
        .collect()
}

/// The audio preview Native Instruments tools keep next to a preset: `.previews/<file>.ogg`
/// and its variants.
pub fn preview_path(path: &str) -> Option<String> {
    let preset = Path::new(path);
    let directory = preset.parent()?;
    let previews = directory.join(".previews");
    let (file, stem) = (file_name(path), stem(path));
    let candidates: [PathBuf; 5] = [
        previews.join(format!("{file}.ogg")),
        previews.join(format!("{stem}.ogg")),
        previews.join(format!("{file}.wav")),
        previews.join(format!("{stem}.wav")),
        directory.join(format!("{stem}.ogg")),
    ];
    candidates.iter().find(|candidate| is_readable_file(candidate)).map(|candidate| candidate.to_string_lossy().into_owned())
}

/// Splits a metadata chunk into its version word (when it has one) and its MessagePack bytes.
pub(crate) fn chunk_content(bytes: &[u8]) -> (Option<u32>, &[u8]) {
    // NKS metadata normally starts with a little-endian version word.
    if bytes.len() >= 5 && little32(bytes, 0) <= 1024 {
        (Some(little32(bytes, 0)), &bytes[4..])
    } else {
        (None, bytes)
    }
}

fn decode_map(content: &[u8]) -> Result<Map<String, Value>> {
    let mut decoder = Decoder::new(content);
    match decoder.decode()? {
        Value::Object(fields) if decoder.is_at_end() => Ok(fields),
        _ => Err(Error::invalid("Metadata must contain one complete MessagePack map.")),
    }
}

impl NksMetadata {
    /// Reads the metadata of the preset at `path`.
    ///
    /// Fails when the file is not a well-formed NKS RIFF container. A metadata chunk that
    /// cannot be decoded is reported in `warnings` and the rest is still returned.
    pub fn read(path: &str) -> Result<NksMetadata> {
        let mut file = File::open(path).map_err(|error| Error::io("read", Path::new(path), &error))?;
        let file_size = file.metadata().map_err(|error| Error::io("read", Path::new(path), &error))?.len();
        if file_size < 12 {
            return Err(Error::invalid("The NKS RIFF header is truncated."));
        }
        const CHANGED: &str = "The NKS file changed or was truncated while reading.";
        let header = read_exact_at(&mut file, 0, 12, CHANGED)?;
        if &header[0..4] != b"RIFF" {
            return Err(Error::invalid("This is not a RIFF NKS preset."));
        }
        let form = tag(&header[8..12]);
        if !["NIKS", "NKSF", "NKSX"].contains(&form.as_str()) {
            return Err(Error::invalid(format!("Unsupported NKS RIFF form: {form}.")));
        }
        let riff_end = u64::from(little32(&header, 4)) + 8;
        if riff_end < 12 || riff_end > file_size {
            return Err(Error::invalid("The NKS container length exceeds the file bounds."));
        }

        let mut parsed: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        let (mut warnings, mut chunks, mut chunk_versions) = (Vec::new(), Vec::new(), BTreeMap::new());
        let (mut has_plugin_state, mut metadata_bytes, mut offset, mut chunk_count) = (false, 0usize, 12u64, 0);
        if riff_end < file_size {
            warnings.push("Bytes after the declared RIFF container were ignored.".to_string());
        }
        while offset < riff_end {
            chunk_count += 1;
            if chunk_count > 4096 {
                return Err(Error::invalid("Too many NKS chunks."));
            }
            if riff_end - offset < 8 {
                return Err(Error::invalid("An NKS chunk header is truncated."));
            }
            let chunk_header = read_exact_at(&mut file, offset, 8, CHANGED)?;
            let chunk = tag(&chunk_header[0..4]);
            let length = u64::from(little32(&chunk_header, 4));
            let start = offset + 8;
            if length > riff_end - start {
                return Err(Error::invalid(format!("The {chunk} chunk exceeds the NKS container bounds.")));
            }
            let padded_end = start + length + (length & 1);
            if padded_end > riff_end {
                return Err(Error::invalid(format!("The {chunk} chunk is missing its RIFF padding.")));
            }
            chunks.push(chunk.clone());
            if chunk == "PCHK" {
                has_plugin_state = has_plugin_state || length > 0;
            } else if ["NISI", "NICA", "PLID"].contains(&chunk.as_str()) {
                if parsed.contains_key(&chunk) {
                    warnings.push(format!("Duplicate {chunk} metadata was ignored."));
                } else if length > (METADATA_LIMIT - metadata_bytes) as u64 {
                    warnings.push(format!("The {chunk} chunk exceeds the 8 MiB metadata budget and was skipped."));
                } else {
                    metadata_bytes += length as usize;
                    let bytes = read_exact_at(&mut file, start, length as usize, CHANGED)?;
                    let (version, content) = chunk_content(&bytes);
                    if let Some(version) = version {
                        chunk_versions.insert(chunk.clone(), version);
                        if version != 1 {
                            warnings.push(format!("{chunk} uses metadata version {version}; fields are interpreted conservatively."));
                        }
                    }
                    match decode_map(content) {
                        Ok(fields) => {
                            parsed.insert(chunk, fields);
                        }
                        Err(error) => warnings.push(format!("Could not read {chunk}: {error}")),
                    }
                }
            }
            offset = padded_end;
        }

        if !parsed.contains_key("NISI") {
            warnings.push("No readable NISI summary was found; the file name is used for browsing.".to_string());
        }
        let info = parsed.get("NISI").cloned().unwrap_or_default();
        let field = |key: &str| info.get(key);
        let chain = bank_chain(&info);
        let position = |index: usize| chain.get(index).filter(|value| !value.is_empty()).cloned();
        let characters = || strings(field("characters")).into_iter().chain(strings(field("character")));
        let mappings = parsed.get("NICA");
        Ok(NksMetadata {
            name: nonempty(field("name")).unwrap_or_else(|| stem(path).to_string()),
            path: path.to_string(),
            format: "NKS".to_string(),
            categories: category_paths(field("types")),
            tags: unique(strings(field("modes")).into_iter().chain(strings(field("tags"))).chain(characters()).collect()),
            banks: unique(strings(field("bankchain"))),
            type_paths: type_paths(field("types")),
            characters: unique(characters().chain(strings(field("modes"))).collect()),
            has_plugin_state,
            warnings,
            limitations: LIMITATIONS.iter().map(|text| text.to_string()).collect(),
            direct_au_preset_loading_supported: false,
            chunks,
            chunk_versions,
            author: nonempty(field("author")),
            vendor: nonempty(field("vendor")),
            comment: nonempty(field("comment")),
            device_type: nonempty(field("deviceType")),
            uuid: nonempty(field("UUID")),
            product: position(0),
            bank: position(1),
            sub_bank: position(2),
            plugin_id: parsed.get("PLID").cloned().map(Value::Object),
            controller_pages: mappings.map(controller_pages),
            controller_mappings: mappings.cloned().map(Value::Object),
            preview_path: preview_path(path),
            raw_metadata: Value::Object(info),
        })
    }
}

/// The browsing summary of a preset for the catalogue: `NISI` and `PLID` only.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CatalogSummary {
    /// Preset name.
    pub name: String,
    /// Bank chain positions.
    pub product: String,
    /// Second bank chain position.
    pub bank: String,
    /// Third bank chain position.
    pub sub_bank: String,
    /// Type paths, at most two levels each.
    pub type_paths: Vec<Vec<String>>,
    /// Characters and modes.
    pub characters: Vec<String>,
    /// Whether the preset has controller pages.
    pub has_controls: bool,
    /// `VST3`, `VST2`, `AU` or empty, from the keys of the plug-in identity.
    pub format: String,
    /// Vendor.
    pub vendor: String,
    /// Author.
    pub author: String,
    /// Comment.
    pub comment: String,
    /// `INST` or `FX`.
    pub device_type: String,
}

/// Reads the catalogue summary of the preset at `path`. `NICA` and `PCHK` payloads are skipped
/// by seeking, so thousands of presets index quickly.
pub fn catalog_summary(path: &str) -> Result<CatalogSummary> {
    const TRUNCATED: &str = "The NKS file changed or was truncated while reading.";
    let mut file = File::open(path).map_err(|error| Error::io("read", Path::new(path), &error))?;
    let file_size = file.metadata().map_err(|error| Error::io("read", Path::new(path), &error))?.len();
    if file_size < 12 {
        return Err(Error::invalid("The NKS RIFF header is truncated."));
    }
    let header = read_exact_at(&mut file, 0, 12, TRUNCATED)?;
    if &header[0..4] != b"RIFF" || !["NIKS", "NKSF", "NKSX"].contains(&tag(&header[8..12]).as_str()) {
        return Err(Error::invalid("This is not an NKS preset."));
    }
    let riff_end = file_size.min(u64::from(little32(&header, 4)) + 8);
    let (mut offset, mut chunks, mut has_controls) = (12u64, 0, false);
    let (mut info, mut plugin): (Option<Map<String, Value>>, Option<Map<String, Value>>) = (None, None);
    while offset + 8 <= riff_end && chunks < 4096 && (info.is_none() || plugin.is_none() || !has_controls) {
        chunks += 1;
        let chunk_header = read_exact_at(&mut file, offset, 8, TRUNCATED)?;
        let chunk = tag(&chunk_header[0..4]);
        let (length, start) = (u64::from(little32(&chunk_header, 4)), offset + 8);
        if length > riff_end - start {
            break;
        }
        if chunk == "NICA" {
            has_controls = length > 0;
        } else if (chunk == "NISI" || chunk == "PLID") && length <= METADATA_LIMIT as u64 {
            let bytes = read_exact_at(&mut file, start, length as usize, TRUNCATED)?;
            let (_, content) = chunk_content(&bytes);
            if let Ok(Value::Object(fields)) = Decoder::new(content).decode() {
                if chunk == "NISI" {
                    info = Some(fields);
                } else {
                    plugin = Some(fields);
                }
            }
        }
        offset = start + length + (length & 1);
    }
    let info = info.ok_or_else(|| Error::invalid("No readable NISI summary."))?;
    let chain = bank_chain(&info);
    let keys: Vec<String> = plugin.unwrap_or_default().keys().map(|key| key.to_lowercase()).collect();
    let format = if keys.iter().any(|key| key.starts_with("vst3")) {
        "VST3"
    } else if keys.iter().any(|key| key.starts_with("vst")) {
        "VST2"
    } else if keys.iter().any(|key| key.starts_with("au")) {
        "AU"
    } else {
        ""
    };
    let text = |key: &str| nonempty(info.get(key)).unwrap_or_default();
    Ok(CatalogSummary {
        name: nonempty(info.get("name")).unwrap_or_else(|| stem(path).to_string()),
        product: chain.first().cloned().unwrap_or_default(),
        bank: chain.get(1).cloned().unwrap_or_default(),
        sub_bank: chain.get(2).cloned().unwrap_or_default(),
        type_paths: type_paths(info.get("types")),
        characters: unique(strings(info.get("characters")).into_iter().chain(strings(info.get("character"))).chain(strings(info.get("modes"))).collect()),
        has_controls,
        format: format.to_string(),
        vendor: text("vendor"),
        author: text("author"),
        comment: text("comment"),
        device_type: text("deviceType"),
    })
}

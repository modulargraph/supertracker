//! Loading a selected NKS preset: the plug-in identity, the exact plug-in state bytes and the
//! controller pages (the NKS part of `plugin.loadNKS`).
//!
//! Library scanning uses [`NksMetadata`] and never reads `PCHK`; this reader does, for one
//! preset the user chose. The state is handed to the plug-in host as it is stored: no
//! conversion, decompression or guessed component/controller split. A matching identity alone
//! does not prove the plug-in accepts the state; the host validates after restoring it.

use super::metadata::{little32, nonempty, read_exact_at, tag, NksMetadata};
use crate::error::{Error, Result};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::Path;

/// Which plug-in a preset belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginIdentity {
    /// `vst3` or `vst2`.
    pub format: String,
    /// The VST3 class ID as 32 upper-case hexadecimal digits (the four 32-bit words in order).
    #[serde(rename = "vst3ClassID", skip_serializing_if = "Option::is_none")]
    pub vst3_class_id: Option<String>,
    /// The four-character VST2 identifier as a number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vst2_magic: Option<u32>,
    /// Plug-in name, when the preset states it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Plug-in vendor, when the preset states it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
}

/// One of the eight controls of a controller page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerSlot {
    /// Position on the page, 0..8.
    pub slot: usize,
    /// Control name.
    pub name: String,
    /// Section heading, where the preset gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// The parameter the control addresses: a VST3 parameter ID, or with a VST2 identity the
    /// parameter index of that plug-in. `None` for an empty slot.
    #[serde(rename = "parameterID", skip_serializing_if = "Option::is_none")]
    pub parameter_id: Option<u32>,
}

/// A page of controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControllerPage {
    /// Page number, from 0.
    pub index: usize,
    /// The first section name on the page, or "Page N".
    pub name: String,
    /// The controls.
    #[serde(rename = "parameters")]
    pub slots: Vec<ControllerSlot>,
}

/// A controller slot with the outcome of binding it to a loaded plug-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundSlot {
    /// The slot.
    #[serde(flatten)]
    pub slot: ControllerSlot,
    /// Whether the plug-in exposes the parameter.
    pub mapped: bool,
    /// `mapped`, `unassigned`, `identityMismatch` or `parameterUnavailable`.
    pub mapping_status: String,
}

/// A controller page with bound slots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundPage {
    /// Page number, from 0.
    pub index: usize,
    /// Page name.
    pub name: String,
    /// The controls with their binding.
    pub parameters: Vec<BoundSlot>,
}

/// The JSON-safe summary of a preset: everything but the state bytes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSummary {
    /// The preset file.
    pub path: String,
    /// Preset name.
    pub name: String,
    /// The plug-in.
    pub identity: PluginIdentity,
    /// The version word of the state chunk (1).
    pub state_version: u32,
    /// Length of the state.
    pub state_bytes: usize,
    /// Always `nativePluginState`.
    pub state_encoding: String,
    /// The controller pages.
    pub controller_pages: Vec<ControllerPage>,
    /// What to tell the user.
    pub warnings: Vec<String>,
    /// Always true: the host checks the plug-in after restoring the state.
    pub requires_host_state_validation: bool,
}

/// A preset read for loading.
#[derive(Debug, Clone, PartialEq)]
pub struct NksHostPreset {
    /// The preset file.
    pub path: String,
    /// The plug-in it belongs to.
    pub identity: PluginIdentity,
    /// Preset name.
    pub name: String,
    /// The `PCHK` payload after its version word, exactly as stored.
    pub state: Vec<u8>,
    /// The version word of `PCHK` (1).
    pub state_version: u32,
    /// The controller pages (`NICA` version 1), at most 128 pages of eight slots.
    pub controller_pages: Vec<ControllerPage>,
    /// What to tell the user.
    pub warnings: Vec<String>,
    /// The browsing metadata of the same file.
    pub metadata: NksMetadata,
}

/// The default largest state [`NksHostPreset::read`] loads into memory (256 MiB).
pub const DEFAULT_MAX_STATE_BYTES: usize = 256 * 1024 * 1024;

#[derive(PartialEq)]
struct FileSignature {
    size: u64,
    modified: Option<std::time::SystemTime>,
}

fn signature(path: &str) -> Result<FileSignature> {
    let metadata = fs::metadata(path).map_err(|_| Error::invalid("The NKS preset must be a readable regular file."))?;
    if !metadata.is_file() {
        return Err(Error::invalid("The NKS preset must be a readable regular file."));
    }
    Ok(FileSignature { size: metadata.len(), modified: metadata.modified().ok() })
}

/// A JSON number as an unsigned 32-bit value. With `allow_signed_bits`, negative 32-bit values
/// keep their bit pattern (NKS writers store identity words as signed integers).
fn uint32(value: Option<&Value>, allow_signed_bits: bool) -> Option<u32> {
    let raw = match value? {
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    if !raw.is_finite() || raw.trunc() != raw {
        return None;
    }
    if (0.0..=f64::from(u32::MAX)).contains(&raw) {
        return Some(raw as u32);
    }
    if allow_signed_bits && raw >= f64::from(i32::MIN) && raw < 0.0 {
        return Some(raw as i32 as u32);
    }
    None
}

fn identity(fields: &Map<String, Value>) -> Result<PluginIdentity> {
    let mut class_id = None;
    if let Some(value) = fields.get("VST3.uid") {
        let words = value.as_array().filter(|words| words.len() == 4).ok_or_else(|| Error::invalid("The NKS VST3 class identity must contain four 32-bit words."))?;
        let numbers: Vec<u32> = words
            .iter()
            .map(|word| uint32(Some(word), true).ok_or_else(|| Error::invalid("The NKS VST3 class identity contains an invalid word.")))
            .collect::<Result<_>>()?;
        if numbers.iter().all(|number| *number == 0) {
            return Err(Error::invalid("The NKS VST3 class identity is empty."));
        }
        class_id = Some(numbers.iter().map(|number| format!("{number:08X}")).collect::<String>());
    }
    let magic = uint32(fields.get("VST.magic"), true).filter(|magic| *magic != 0);
    if class_id.is_none() && magic.is_none() {
        return Err(Error::invalid("This preset does not identify a supported VST2 or VST3 plug-in. It can still be browsed."));
    }
    Ok(PluginIdentity {
        format: if class_id.is_some() { "vst3" } else { "vst2" }.to_string(),
        vst3_class_id: class_id,
        vst2_magic: magic,
        name: nonempty(fields.get("pluginName")).or_else(|| nonempty(fields.get("VST3.plugin_name"))),
        vendor: nonempty(fields.get("pluginVendor")),
    })
}

fn pages(fields: &Map<String, Value>, warnings: &mut Vec<String>) -> Vec<ControllerPage> {
    let Some(Value::Array(pages)) = fields.get("ni8") else { return Vec::new() };
    if pages.len() > 128 {
        warnings.push("Only the first 128 NKS controller pages are exposed.".to_string());
    }
    let mut bad_ids = 0;
    let result: Vec<ControllerPage> = pages
        .iter()
        .take(128)
        .enumerate()
        .filter_map(|(index, raw)| {
            let slots = raw.as_array()?;
            if slots.len() > 8 {
                warnings.push(format!("NKS page {} contains more than eight slots; extra slots are not exposed.", index + 1));
            }
            let controls: Vec<ControllerSlot> = slots
                .iter()
                .take(8)
                .enumerate()
                .map(|(slot, raw)| {
                    let empty = Map::new();
                    let fields = raw.as_object().unwrap_or(&empty);
                    let id = uint32(fields.get("id"), false).filter(|id| *id != u32::MAX);
                    if fields.contains_key("id") && id.is_none() {
                        bad_ids += 1;
                    }
                    ControllerSlot {
                        slot,
                        name: nonempty(fields.get("name")).unwrap_or_else(|| format!("Control {}", slot + 1)),
                        section: nonempty(fields.get("section")),
                        parameter_id: id,
                    }
                })
                .collect();
            let name = controls.iter().find_map(|control| control.section.clone()).unwrap_or_else(|| format!("Page {}", index + 1));
            Some(ControllerPage { index, name, slots: controls })
        })
        .collect();
    if bad_ids > 0 {
        warnings.push(format!("{bad_ids} invalid or unassigned controller IDs were left unbound."));
    }
    result
}

impl NksHostPreset {
    /// Reads the preset at `path` with the default state limit.
    pub fn read(path: &str) -> Result<NksHostPreset> {
        NksHostPreset::read_with_limit(path, DEFAULT_MAX_STATE_BYTES)
    }

    /// Reads the preset at `path`, refusing a state larger than `max_state_bytes`.
    ///
    /// Fails when the file has no readable plug-in identity (`PLID` version 1), no state
    /// (`PCHK` version 1), more than one of either, or changes while it is read.
    pub fn read_with_limit(path: &str, max_state_bytes: usize) -> Result<NksHostPreset> {
        if max_state_bytes == 0 || max_state_bytes > 512 * 1024 * 1024 {
            return Err(Error::invalid("Invalid NKS state memory limit."));
        }
        let before = signature(path)?;
        if before.size < 12 || before.size > 512 * 1024 * 1024 {
            return Err(Error::invalid("The NKS preset is truncated or exceeds the 512 MiB file limit."));
        }
        let metadata = NksMetadata::read(path)?;
        let plid = metadata.plugin_id.as_ref().and_then(Value::as_object).ok_or_else(|| Error::invalid("This preset has no readable plug-in identity (PLID)."))?;
        if metadata.chunk_versions.get("PLID") != Some(&1) {
            return Err(Error::invalid("This preset uses an unrecognized PLID version. It can be browsed but cannot be loaded directly yet."));
        }
        let mut warnings = metadata.warnings.clone();
        let identity = identity(plid)?;
        if identity.vst3_class_id.is_none() {
            warnings.push("This preset identifies a VST2 plug-in. It loads into the vendor's VST2-compatible VST3 release.".to_string());
        }
        const TRUNCATED: &str = "The NKS preset was truncated while reading.";
        let mut file = File::open(path).map_err(|error| Error::io("read", Path::new(path), &error))?;
        let header = read_exact_at(&mut file, 0, 12, TRUNCATED)?;
        if &header[0..4] != b"RIFF" || !["NIKS", "NKSF", "NKSX"].contains(&tag(&header[8..12]).as_str()) {
            return Err(Error::invalid("This file is not a supported NKS RIFF container."));
        }
        let end = u64::from(little32(&header, 4)) + 8;
        if end < 12 || end > before.size {
            return Err(Error::invalid("The NKS container length is invalid."));
        }
        let (mut position, mut chunks, mut identities, mut version) = (12u64, 0, 0, 0u32);
        let mut state: Option<Vec<u8>> = None;
        while position < end {
            chunks += 1;
            if chunks > 4096 || end - position < 8 {
                return Err(Error::invalid("The NKS chunk table is invalid or exceeds its limit."));
            }
            let chunk = read_exact_at(&mut file, position, 8, TRUNCATED)?;
            let name = tag(&chunk[0..4]);
            let length = u64::from(little32(&chunk, 4));
            let start = position + 8;
            if length > end - start || length + (length & 1) > end - start {
                return Err(Error::invalid(format!("The NKS {name} chunk exceeds the container bounds.")));
            }
            if name == "PLID" {
                identities += 1;
            }
            if name == "PCHK" {
                if state.is_some() {
                    return Err(Error::invalid("The preset contains multiple plug-in states. Direct loading would be ambiguous."));
                }
                if length <= 4 {
                    return Err(Error::invalid("The NKS preset has no saved plug-in state."));
                }
                if length - 4 > max_state_bytes as u64 {
                    return Err(Error::invalid("The preset's plug-in state exceeds the configured memory limit."));
                }
                version = little32(&read_exact_at(&mut file, start, 4, TRUNCATED)?, 0);
                if version != 1 {
                    return Err(Error::invalid(format!("PCHK state version {version} is not supported. The state was not sent to a plug-in.")));
                }
                let mut bytes = vec![0u8; (length - 4) as usize];
                use std::io::{Read, Seek, SeekFrom};
                file.seek(SeekFrom::Start(start + 4)).and_then(|_| file.read_exact(&mut bytes)).map_err(|_| Error::invalid(TRUNCATED))?;
                state = Some(bytes);
            }
            position = start + length + (length & 1);
        }
        if identities != 1 {
            return Err(Error::invalid("The preset contains multiple plug-in identities. Direct loading would be ambiguous."));
        }
        let state = state.ok_or_else(|| Error::invalid("The NKS preset has no PCHK plug-in state."))?;
        if before != signature(path)? {
            return Err(Error::invalid("The NKS preset changed while it was being read. Try loading it again."));
        }
        let controller_pages = if metadata.chunk_versions.get("NICA") == Some(&1) {
            let empty = Map::new();
            pages(metadata.controller_mappings.as_ref().and_then(Value::as_object).unwrap_or(&empty), &mut warnings)
        } else {
            if metadata.controller_mappings.is_some() {
                warnings.push("Controller mappings use an unrecognized NICA version and were not bound.".to_string());
            }
            Vec::new()
        };
        Ok(NksHostPreset { path: crate::util::normalized(path), identity, name: metadata.name.clone(), state, state_version: version, controller_pages, warnings, metadata })
    }

    /// The summary the page receives. It never contains the state bytes.
    pub fn summary(&self) -> PresetSummary {
        PresetSummary {
            path: self.path.clone(),
            name: self.name.clone(),
            identity: self.identity.clone(),
            state_version: self.state_version,
            state_bytes: self.state.len(),
            state_encoding: "nativePluginState".to_string(),
            controller_pages: self.controller_pages.clone(),
            warnings: self.warnings.clone(),
            requires_host_state_validation: true,
        }
    }

    /// Whether the preset belongs to the VST3 class `vst3_class_id` (32 hexadecimal digits):
    /// the exact class, or for a VST2 preset the class a vendor ships as that plug-in's
    /// VST2-compatible VST3 (see [`is_vst2_compatible`]).
    pub fn matches_vst3(&self, vst3_class_id: &str) -> bool {
        let wanted = vst3_class_id.to_uppercase();
        if let Some(exact) = &self.identity.vst3_class_id {
            return *exact == wanted;
        }
        self.identity.vst2_magic.is_some_and(|magic| is_vst2_compatible(&wanted, magic))
    }

    /// The controller pages bound to the parameters a loaded VST3 component exposes after its
    /// state was restored. Order is never read as a parameter ID: a slot is `mapped` only when
    /// the identity matches and the plug-in has that parameter ID.
    pub fn validated_mappings(&self, vst3_class_id: &str, parameter_ids: &HashSet<u32>) -> Vec<BoundPage> {
        let identity_matches = self.matches_vst3(vst3_class_id);
        self.controller_pages
            .iter()
            .map(|page| BoundPage {
                index: page.index,
                name: page.name.clone(),
                parameters: page
                    .slots
                    .iter()
                    .map(|slot| {
                        let mapped = identity_matches && slot.parameter_id.is_some_and(|id| parameter_ids.contains(&id));
                        let status = match slot.parameter_id {
                            None => "unassigned",
                            Some(_) if !identity_matches => "identityMismatch",
                            Some(_) if mapped => "mapped",
                            Some(_) => "parameterUnavailable",
                        };
                        BoundSlot { slot: slot.clone(), mapped, mapping_status: status.to_string() }
                    })
                    .collect(),
            })
            .collect()
    }
}

/// Whether the VST3 class `class_id` is the VST2-compatible class of the VST2 plug-in `vst2_magic`.
///
/// Steinberg's migration convention stores "VST" followed by the four-character VST2 ID in the
/// class UID (Native Instruments, Waves, iZotope); Arturia stores the ID in the third word.
/// Such classes accept the VST2 plug-in's NKS state and parameter IDs.
pub fn is_vst2_compatible(class_id: &str, vst2_magic: u32) -> bool {
    let class_id = class_id.to_uppercase();
    if class_id.len() != 32 || !class_id.is_ascii() {
        return false;
    }
    let magic = format!("{vst2_magic:08X}");
    let steinberg = &class_id[0..6] == "565354" && class_id[6..14] == magic;
    let arturia = &class_id[0..8] == "41727475" && class_id[16..24] == magic;
    steinberg || arturia
}

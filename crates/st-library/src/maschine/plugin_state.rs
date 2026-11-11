//! The saved state of a plug-in hosted in a Maschine group (a `PluginHost` Module).
//!
//! The member order is: four Boolean parameters, the unique plug-in identity
//! (`Vendor:VST:INSTR:<decimal VST2 ID>`), the display name, the plug-in chunk (version, type,
//! size, opaque bytes), the selected preset index, then parameter mapping and window members
//! that are not read. The state stays opaque: nothing here loads a plug-in or converts state.
//!
//! Maschine hosts VST2 plug-ins. SuperTracker does not load VST2, so an imported group keeps
//! the identity and the state bytes for inspection (and for a vendor's VST2-compatible VST3),
//! and reports the pad as needing its plug-in replaced.

use super::archive::{invalid, Archive, Module};
use crate::error::Result;
use serde_json::{json, Value};

/// A decoded plug-in host record.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginState {
    /// The identity as stored, for example `Native Instruments:VST:INSTR:1315523937`.
    pub unique_plugin_id: String,
    /// Display name.
    pub plugin_name: String,
    /// The VST2 unique ID, when the identity names a VST plug-in with a numeric ID.
    pub vst2_unique_id: Option<u32>,
    /// Instrument (`INSTR`) or effect (`FX`), when the identity says.
    pub is_instrument: Option<bool>,
    /// Version of the chunk envelope (1).
    pub chunk_version: u32,
    /// Chunk type, 0..=7 (1 is a VST2 bank chunk).
    pub chunk_type: u32,
    /// The plug-in's own state bytes.
    pub payload: Vec<u8>,
    /// The selected preset.
    pub preset_index: u32,
    /// The four host flags.
    pub flags: Vec<bool>,
}

const MAXIMUM_STATE_BYTES: usize = 32 * 1024 * 1024;

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
    fn byte(&mut self) -> Result<u8> {
        let value = *self.bytes.get(self.position).ok_or_else(|| invalid("truncated PluginHost record."))?;
        self.position += 1;
        Ok(value)
    }
    fn integer(&mut self) -> Result<u64> {
        let width = self.byte()? as usize;
        if width > 4 || width > self.remaining() || (width > 0 && self.bytes[self.position + width - 1] == 0) {
            return Err(invalid("non-canonical PluginHost archive integer."));
        }
        let value = self.bytes[self.position..self.position + width].iter().enumerate().fold(0u64, |value, (index, byte)| value | u64::from(*byte) << (8 * index));
        self.position += width;
        Ok(value)
    }
    fn word(&mut self) -> Result<u32> {
        let bytes = self.bytes.get(self.position..self.position + 4).ok_or_else(|| invalid("truncated plug-in chunk header."))?;
        self.position += 4;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
    fn string(&mut self, maximum: usize) -> Result<String> {
        let length = self.integer()? as usize;
        if length > maximum || length > self.remaining() {
            return Err(invalid("PluginHost string exceeds its bounds."));
        }
        let data = &self.bytes[self.position..self.position + length];
        let text = std::str::from_utf8(data).ok().filter(|_| !data.contains(&0)).ok_or_else(|| invalid("PluginHost string is not valid UTF-8."))?;
        self.position += length;
        Ok(text.to_string())
    }
}

impl PluginState {
    /// Reads the state of a `PluginHost` Module of `archive`.
    pub fn read(archive: &Archive, module: &Module) -> Result<PluginState> {
        if module.kind != "PluginHost" || module.end < module.body_offset || module.end > archive.bytes.len() {
            return Err(invalid("the selected module is not a bounded PluginHost record."));
        }
        PluginState::decode(&archive.bytes[module.body_offset..module.end], module.object_id, archive.modern(), module.version as i64, MAXIMUM_STATE_BYTES)
    }

    /// Decodes a module body. `modern` is the archive's parameter form; `module_version` 0
    /// (Maschine 1.x) always uses the three-ID form. Fails on any truncated or inconsistent
    /// field, and when the state is larger than `max_state_bytes`.
    pub fn decode(body: &[u8], object_id: i64, modern: bool, module_version: i64, max_state_bytes: usize) -> Result<PluginState> {
        if module_version != 0 && module_version != 1 {
            return Err(invalid(format!("PluginHost version {module_version} has no verified state layout.")));
        }
        let modern = modern && module_version == 1;
        if max_state_bytes == 0 || max_state_bytes > MAXIMUM_STATE_BYTES || body.len() > MAXIMUM_STATE_BYTES || object_id <= 0 || object_id > i64::from(u32::MAX) - 16 {
            return Err(invalid("PluginHost object or state bounds are invalid."));
        }
        let mut reader = Reader { bytes: body, position: 0 };
        let mut expected = object_id as u64;
        let mut flags = Vec::with_capacity(4);
        for _ in 0..4 {
            for _ in 0..if modern { 4 } else { 3 } {
                expected += 1;
                if reader.integer()? != expected {
                    return Err(invalid("PluginHost parameter identities do not match their member order."));
                }
            }
            if modern && reader.byte()? != 0 {
                return Err(invalid("PluginHost parameter base tracking is unsupported."));
            }
            let mut first = 0;
            for index in 0..5 {
                let value = reader.integer()?;
                if value > 1 {
                    return Err(invalid("PluginHost Boolean parameter is invalid."));
                }
                if index == 0 {
                    first = value;
                }
            }
            flags.push(first != 0);
        }
        let unique_plugin_id = reader.string(4096)?;
        let plugin_name = reader.string(4096)?;
        if unique_plugin_id.is_empty() || plugin_name.is_empty() {
            return Err(invalid("PluginHost identity or display name is empty."));
        }
        // The first plug-in chunk of an archive introduces its class (two zero bytes); later
        // PluginHosts reuse that registration.
        if reader.remaining() >= 2 && reader.bytes[reader.position] == 0 && reader.bytes[reader.position + 1] == 0 {
            reader.position += 2;
        }
        let length = reader.integer()? as usize;
        if length < 12 || length - 12 > max_state_bytes || length > reader.remaining() {
            return Err(invalid("PluginHost chunk is truncated or exceeds the state limit."));
        }
        let chunk_end = reader.position + length;
        let (chunk_version, chunk_type, size) = (reader.word()?, reader.word()?, reader.word()?);
        if chunk_version != 1 {
            return Err(invalid(format!("NI plug-in chunk version {chunk_version} is unsupported.")));
        }
        if chunk_type > 7 || size == 0 || size as usize != length - 12 {
            return Err(invalid("NI plug-in chunk type or payload size is invalid."));
        }
        let payload = reader.bytes[reader.position..chunk_end].to_vec();
        reader.position = chunk_end;
        let preset_index = reader.integer()? as u32;
        // Only the exact role and technology tokens establish what the plug-in is: never its name.
        let fields: Vec<&str> = unique_plugin_id.splitn(4, ':').collect();
        let (mut vst2_unique_id, mut is_instrument) = (None, None);
        if fields.len() == 4 && !fields[0].is_empty() {
            is_instrument = match fields[2] {
                "INSTR" => Some(true),
                "FX" => Some(false),
                _ => None,
            };
            if fields[1] == "VST" && is_instrument.is_some() && !fields[3].is_empty() && fields[3].bytes().all(|byte| byte.is_ascii_digit()) {
                vst2_unique_id = fields[3].parse::<u32>().ok().filter(|id| *id != 0);
            }
        }
        Ok(PluginState { unique_plugin_id, plugin_name, vst2_unique_id, is_instrument, chunk_version, chunk_type, payload, preset_index, flags })
    }

    /// The record without its state bytes, for the import report.
    pub fn summary(&self) -> Value {
        let mut result = json!({
            "uniquePluginID": self.unique_plugin_id, "pluginName": self.plugin_name, "chunkVersion": self.chunk_version, "chunkType": self.chunk_type,
            "stateBytes": self.payload.len(), "presetIndex": self.preset_index, "stateEncoding": "NI.HOSTING.PluginChunk", "requiresHostStateValidation": true, "hostFlags": self.flags,
        });
        if let Some(id) = self.vst2_unique_id {
            result["vst2UniqueID"] = json!(id);
        }
        if let Some(instrument) = self.is_instrument {
            result["kind"] = json!(if instrument { "instrument" } else { "effect" });
        }
        result
    }

    /// The component identifier of the VST2 plug-in (`vst2:4E694D61`), when the identity has one.
    pub fn component(&self) -> Option<String> {
        self.vst2_unique_id.map(|id| format!("vst2:{id:08X}"))
    }
}

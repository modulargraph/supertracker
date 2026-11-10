//! NKS presets: metadata, loading, the preset catalogue and product artwork.

pub mod artwork;
pub mod catalog;
pub mod metadata;
pub mod msgpack;
pub mod preset;
pub mod sqlite;

pub use artwork::{ArtMatch, NksArtwork};
pub use catalog::{BrowseFacet, NksBrowseResponse, NksCatalog, NksStatus, Registration};
pub use metadata::{catalog_summary, CatalogSummary, MetadataPage, NksMetadata};
pub use preset::{is_vst2_compatible, BoundPage, BoundSlot, ControllerPage, ControllerSlot, NksHostPreset, PluginIdentity, PresetSummary};

/// Builders of small NKS files for tests.
#[doc(hidden)]
pub mod fixtures {
    use serde_json::Value;

    /// A little-endian 32-bit word.
    pub fn word(value: u32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    /// A RIFF chunk with its padding byte.
    pub fn chunk(name: &str, content: &[u8]) -> Vec<u8> {
        let mut data = name.as_bytes().to_vec();
        data.extend(word(content.len() as u32));
        data.extend_from_slice(content);
        if content.len() % 2 == 1 {
            data.push(0);
        }
        data
    }

    /// A `RIFF`/`NIKS` container around `chunks`.
    pub fn riff(chunks: &[u8]) -> Vec<u8> {
        let mut data = b"RIFF".to_vec();
        data.extend(word(chunks.len() as u32 + 4));
        data.extend_from_slice(b"NIKS");
        data.extend_from_slice(chunks);
        data
    }

    /// A version-1 metadata chunk holding `value` as MessagePack.
    pub fn metadata_chunk(name: &str, value: &Value) -> Vec<u8> {
        let mut content = word(1);
        content.extend(super::msgpack::pack(value));
        chunk(name, &content)
    }

    /// A complete preset: summary, optional identity and controller pages, and state bytes.
    pub fn preset(summary: &Value, plugin: Option<&Value>, controls: Option<&Value>, state: Option<&[u8]>) -> Vec<u8> {
        let mut chunks = metadata_chunk("NISI", summary);
        if let Some(controls) = controls {
            chunks.extend(metadata_chunk("NICA", controls));
        }
        if let Some(plugin) = plugin {
            chunks.extend(metadata_chunk("PLID", plugin));
        }
        if let Some(state) = state {
            let mut content = word(1);
            content.extend_from_slice(state);
            chunks.extend(chunk("PCHK", &content));
        }
        riff(&chunks)
    }
}

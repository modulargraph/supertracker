//! Payloads and results of the bridge commands this crate serves (`library.*`, `kit.*`).
//!
//! Field names are the ones the page uses (camelCase). Requests ignore unknown fields and
//! treat a missing or `null` field as its default; results leave out fields that do not apply.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

fn null_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(deserializer: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn default_true() -> bool {
    true
}

fn true_unless_false<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(true))
}

/// `offset`/`limit` as the page sends them (any JSON number), within `0..` and `1..=1000`.
pub(crate) fn page(offset: Option<f64>, limit: Option<f64>) -> (usize, usize) {
    let whole = |value: Option<f64>, fallback: f64| value.filter(|value| value.is_finite()).unwrap_or(fallback).trunc();
    (whole(offset, 0.0).clamp(0.0, 1.0e9) as usize, whole(limit, 200.0).clamp(1.0, 1000.0) as usize)
}

// ---------------------------------------------------------------------------------------------
// Requests

/// The payload of a command that takes none. Whatever was sent is ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Empty;

impl<'de> Deserialize<'de> for Empty {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| Empty)
    }
}

/// `library.scan`: the folders to scan (and add to the library), or none for every root.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScanRequest {
    /// Up to 128 folders. Absent: rescan all library roots.
    pub roots: Option<Vec<String>>,
}

/// `library.diagnostics`: the folders to probe, or none for every root.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiagnosticsRequest {
    /// Up to 128 folders.
    pub roots: Option<Vec<String>>,
}

/// A numeric range filter (`bpm`, `duration`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NumericFilter {
    /// Lower bound, inclusive.
    pub min: Option<f64>,
    /// Upper bound, inclusive.
    pub max: Option<f64>,
    /// Also match items whose value is unknown. Alone: only those.
    #[serde(deserialize_with = "null_default")]
    pub include_unknown: bool,
}

/// The structured filters of `library.search`. Values of one facet are alternatives (OR);
/// different facets narrow each other (AND).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchFilters {
    /// Categories ("Kick", "Bass" ...).
    #[serde(deserialize_with = "null_default")]
    pub categories: Vec<String>,
    /// Collections ("Loopcloud", "Maschine expansions", "Native Instruments", "User libraries").
    #[serde(deserialize_with = "null_default")]
    pub collections: Vec<String>,
    /// Another name for `collections`.
    #[serde(deserialize_with = "null_default")]
    pub sources: Vec<String>,
    /// Library root paths (the Locations filter).
    #[serde(deserialize_with = "null_default")]
    pub roots: Vec<String>,
    /// Pack names.
    #[serde(deserialize_with = "null_default")]
    pub packs: Vec<String>,
    /// Keys ("A minor"; enharmonic spellings match) or `unknown`.
    #[serde(deserialize_with = "null_default")]
    pub keys: Vec<String>,
    /// `oneshot`, `loop`, `unknown`.
    #[serde(deserialize_with = "null_default")]
    pub sample_types: Vec<String>,
    /// File extensions.
    #[serde(deserialize_with = "null_default")]
    pub extensions: Vec<String>,
    /// Tags.
    #[serde(deserialize_with = "null_default")]
    pub tags: Vec<String>,
    /// Tempo range.
    #[serde(deserialize_with = "null_default")]
    pub bpm: NumericFilter,
    /// Length range in seconds.
    #[serde(deserialize_with = "null_default")]
    pub duration: NumericFilter,
}

/// `library.search`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchRequest {
    /// Free text. Words must all match; `tag:`, `pack:`, `bpm:`, `key:`, `ext:`, `kind:` and
    /// `category:` prefixes filter; quotes keep spaces (`pack:"Orbit Pack"`).
    #[serde(deserialize_with = "null_default")]
    pub query: String,
    /// `all` (samples and presets, not kits), `sample`, `nks` or `kit`.
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
    /// Structured filters.
    #[serde(deserialize_with = "null_default")]
    pub filters: SearchFilters,
    /// Only favorites.
    #[serde(deserialize_with = "null_default")]
    pub favorites: bool,
    /// First result.
    pub offset: Option<f64>,
    /// Page size, 1..=1000 (200 when absent).
    pub limit: Option<f64>,
    /// Leave out sounds of folders that are not reachable right now.
    #[serde(deserialize_with = "null_default")]
    pub hide_offline: bool,
    /// One category, when `filters.categories` is empty (`all` means none).
    pub category: Option<String>,
    /// One pack, when `filters.packs` is empty (`all` means none).
    pub pack: Option<String>,
}

/// `library.metadata.edit`: user labels for up to 500 indexed sounds. Source files are never touched.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MetadataEditRequest {
    /// The sounds.
    #[serde(deserialize_with = "null_default")]
    pub paths: Vec<String>,
    /// `category`, `tags`, `addTags`, `bpm`, `key`, `sampleType`; `null` restores the inferred value.
    pub changes: Option<Map<String, Value>>,
}

/// `library.favorite`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteRequest {
    /// The sound, preset or kit.
    #[serde(default, deserialize_with = "null_default")]
    pub path: String,
    /// Absent: true.
    #[serde(default = "default_true", deserialize_with = "true_unless_false")]
    pub favorite: bool,
}

/// `library.state.set`: what the browser remembers (its query, filters, recent searches).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StateRequest {
    /// A JSON object of at most 256 KiB.
    #[serde(default)]
    pub state: Value,
}

/// A request that names one file or folder (`library.removeRoot`, `library.nks`, `library.kit`, `kit.read`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PathRequest {
    /// The path.
    #[serde(default)]
    pub path: Option<String>,
}

/// `library.nks.browse`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NksBrowseRequest {
    /// `instrument` (default) or `effect`.
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
    /// Free text; every word must match.
    #[serde(deserialize_with = "null_default")]
    pub query: String,
    /// Product.
    pub product: Option<String>,
    /// Bank of the product.
    pub bank: Option<String>,
    /// Sub-bank; ignored without a bank.
    pub sub_bank: Option<String>,
    /// Vendor; narrows the product list.
    pub vendor: Option<String>,
    /// Types (any).
    #[serde(deserialize_with = "null_default")]
    pub types: Vec<String>,
    /// Sub-types as "Type<US>Sub-type" (any).
    #[serde(deserialize_with = "null_default")]
    pub subtypes: Vec<String>,
    /// Characters (all).
    #[serde(deserialize_with = "null_default")]
    pub characters: Vec<String>,
    /// Only favorites.
    #[serde(deserialize_with = "null_default")]
    pub favorites: bool,
    /// First result.
    pub offset: Option<f64>,
    /// Page size, 1..=1000 (200 when absent).
    pub limit: Option<f64>,
}

/// `library.kits.browse`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KitsBrowseRequest {
    /// Free text; every word must match.
    #[serde(deserialize_with = "null_default")]
    pub query: String,
    /// The user's own kits and groups (true) or library content (false).
    #[serde(deserialize_with = "null_default")]
    pub user: bool,
    /// Product (expansion).
    pub product: Option<String>,
    /// Bank; ignored without a product.
    pub bank: Option<String>,
    /// Sub-bank; ignored without a bank.
    pub sub_bank: Option<String>,
    /// Types (any).
    #[serde(deserialize_with = "null_default")]
    pub types: Vec<String>,
    /// Sub-types as "Type<US>Sub-type" (any).
    #[serde(deserialize_with = "null_default")]
    pub subtypes: Vec<String>,
    /// Characters (all).
    #[serde(deserialize_with = "null_default")]
    pub characters: Vec<String>,
    /// Only favorites.
    #[serde(deserialize_with = "null_default")]
    pub favorites: bool,
    /// First result.
    pub offset: Option<f64>,
    /// Page size, 1..=1000 (200 when absent).
    pub limit: Option<f64>,
}

/// `library.nks.artwork`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArtworkRequest {
    /// Artwork keys from browse results (`art`, `productArt`); at most 400 are answered.
    #[serde(deserialize_with = "null_default")]
    pub keys: Vec<String>,
    /// `tile` (default), `banner` or `logo`.
    pub variant: Option<String>,
}

/// `kit.detail`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KitDetailRequest {
    /// The kit package or group file.
    pub path: Option<String>,
    /// Component identifiers of the installed plug-ins (the host fills this in from its plug-in
    /// list). Absent: plug-in availability is not judged.
    pub available_components: Option<Vec<String>>,
}

/// `kit.save`: saves a group of the session as a kit package in the user's kit folder.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KitSaveRequest {
    /// The session, as the page publishes it (plug-in states captured).
    pub project: Value,
    /// The group to save.
    #[serde(rename = "groupID")]
    pub group_id: Option<String>,
    /// Save As: the kit's name. An existing kit of that name is reported unless `overwrite`.
    pub name: Option<String>,
    /// Save: the kit's own package. Only packages in the user's kit folder are replaced.
    pub path: Option<String>,
    /// Replace an existing package.
    #[serde(deserialize_with = "null_default")]
    pub overwrite: bool,
    /// Copy samples into the package. Absent: true.
    #[serde(default = "default_true", deserialize_with = "true_unless_false")]
    pub copy_samples: bool,
    /// An audio preview to copy into the package (the host renders one before it calls).
    pub preview_path: Option<String>,
    /// The session's project file, for relative sample paths.
    pub project_path: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Results and event payloads

/// One library folder (`library.roots`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    /// The folder.
    pub path: String,
    /// Its name.
    pub name: String,
    /// `user`, `configured`, or the standard location it is ("Apple Loops", "Native Instruments" ...).
    pub source: String,
    /// Whether the folder exists right now.
    pub available: bool,
    /// `unscanned`, `pending`, `scanning`, `complete`, `empty`, `error` or `cancelled`.
    pub scan_status: String,
    /// Why the last scan of this folder failed.
    pub scan_errors: Vec<String>,
    /// Indexed files below it.
    pub indexed_count: usize,
}

/// The outcome of scanning one root.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RootReport {
    /// The folder.
    pub path: String,
    /// `pending`, `scanning`, `complete`, `empty`, `error` or `cancelled`.
    pub status: String,
    /// Entries looked at.
    pub visited: u64,
    /// Supported files found.
    pub matched: u64,
    /// Entries skipped (hidden, links, software bundles, empty files).
    pub skipped: u64,
    /// Up to six messages.
    pub errors: Vec<String>,
}

/// A root as named in a scan reply.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanRoot {
    /// The folder.
    pub path: String,
    /// Its name.
    pub name: String,
    /// See [`RootInfo::source`].
    pub source: String,
    /// Whether the folder exists right now.
    pub available: bool,
}

/// The state of the index: the result of `library.scan`, `library.status` and `library.cancel`,
/// and the payload of the `library.progress` and `library.results` events.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// Whether a scan is running.
    pub scanning: bool,
    /// Indexed files.
    pub count: usize,
    /// Messages of the last scan (up to 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
    /// Per-root outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_reports: Option<Vec<RootReport>>,
    /// The roots a scan was started for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<Vec<ScanRoot>>,
    /// Identity of the scan.
    #[serde(rename = "scanID", skip_serializing_if = "Option::is_none")]
    pub scan_id: Option<String>,
    /// The root being scanned (empty when the scan has finished).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Supported files found so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned: Option<u64>,
    /// Entries skipped so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<u64>,
    /// The scan was cancelled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
    /// The saved index was loaded (first `library.results` after start).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<bool>,
    /// The saved index could not be used and is being rebuilt by a scan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuilding: Option<bool>,
    /// A library folder became available or unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volumes: Option<bool>,
    /// Where the diagnostics of the last scan are written (`library.status`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics_path: Option<String>,
}

/// What can be edited about a sound.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableMetadata {
    /// Category.
    pub category: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Tempo; 0 when unknown.
    pub bpm: f64,
    /// Key; empty when unknown.
    pub key: String,
    /// `oneshot`, `loop` or `unknown`.
    pub sample_type: String,
}

/// One indexed file in search results.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    /// The path (the item's identity).
    pub id: String,
    /// The file (for a `.stkit` kit, the package).
    pub path: String,
    /// Display name.
    pub name: String,
    /// `sample`, `nks` or `kit`.
    pub kind: String,
    /// Category.
    pub category: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Pack.
    pub pack: String,
    /// Whether it is a favorite.
    pub favorite: bool,
    /// Size in bytes.
    pub size: u64,
    /// The folder that holds it.
    pub folder: String,
    /// Lower-case extension.
    pub extension: String,
    /// Where the labels came from, in words.
    pub metadata_source: String,
    /// The library root it was found under.
    pub root: String,
    /// One of the four collections.
    pub collection: String,
    /// `oneshot`, `loop` or `unknown`.
    pub sample_type: String,
    /// `known`, `pending` (its header is being read) or `unknown`.
    pub duration_status: String,
    /// The user's own labels (only the edited fields).
    pub user_metadata: Value,
    /// What the path and file say, without the user's labels.
    pub inferred_metadata: EditableMetadata,
    /// Tempo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    /// Key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Audio preview (presets and kits).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_path: Option<String>,
    /// Length in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// `audio header` when `duration` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_source: Option<String>,
    /// Kits: `supertracker-kit` or `maschine-group`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kit_format: Option<String>,
    /// Kits: the collection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Kits: the pack.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expansion: Option<String>,
    /// Kits: the kit type.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kit_type: Option<String>,
    /// Kits: one of the user's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<bool>,
    /// Kits: loaded pads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_count: Option<u32>,
    /// Kits: why the header could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_error: Option<String>,
    /// `false` when the item's library folder is not reachable right now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    /// The external volume an unavailable item is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
}

/// One value of a facet with the number of results choosing it would give.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Facet {
    /// The value to send back as a filter.
    pub value: String,
    /// The same.
    pub name: String,
    /// What to show.
    pub label: String,
    /// Results with this value when the other filters stay as they are.
    pub count: usize,
}

/// How many of the matching sounds have a known length.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct DurationCoverage {
    /// With a length.
    pub known: usize,
    /// Without one.
    pub unknown: usize,
    /// Headers being read.
    pub pending: usize,
}

/// A library folder that is not reachable right now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OfflineRoot {
    /// The folder.
    pub path: String,
    /// Its name.
    pub name: String,
    /// The external volume it is on, or empty.
    pub volume: String,
    /// Indexed sounds below it that match the search.
    pub matching: usize,
}

/// The result of `library.search`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    /// The page of results.
    pub items: Vec<LibraryItem>,
    /// All matching results.
    pub total: usize,
    /// Whether a scan is running.
    pub scanning: bool,
    /// The request was superseded by a newer search; nothing else is filled in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
    /// `facets.categories`.
    pub categories: Vec<Facet>,
    /// `facets.packs`.
    pub packs: Vec<Facet>,
    /// `categories`, `collections` (and `sources`), `roots`, `packs`, `keys`, `sampleTypes`,
    /// `extensions`, `tags`, `bpm` and `duration` (known/unknown), each up to 1000 values.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub facets: BTreeMap<String, Vec<Facet>>,
    /// Facets with more than 1000 values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets_truncated: Option<Vec<String>>,
    /// Length coverage of the matching sounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_coverage: Option<DurationCoverage>,
    /// The request's offset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// The request's page size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Messages of the last scan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
    /// Per-root outcomes of the last scan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_reports: Option<Vec<RootReport>>,
    /// Library folders that are not reachable right now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline: Option<Vec<OfflineRoot>>,
    /// The request's `hideOffline`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_offline: Option<bool>,
}

/// The result of `library.metadata.edit`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetadataEditResponse {
    /// The edited items as they are now.
    pub items: Vec<LibraryItem>,
    /// How many items were edited.
    pub updated: usize,
}

/// The result of `library.favorite` and the payload of the `library.favorite` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FavoriteResponse {
    /// The normalized path.
    pub path: String,
    /// Its new state.
    pub favorite: bool,
}

/// File attributes in a folder probe.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeAttributes {
    /// A directory.
    pub is_directory: bool,
    /// A regular file.
    pub is_regular_file: bool,
    /// A symbolic link.
    pub is_symbolic_link: bool,
    /// Its name starts with a dot.
    pub is_hidden: bool,
    /// It can be opened for reading.
    pub is_readable: bool,
}

/// One entry of a probed folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeEntry {
    /// File name.
    pub name: String,
    /// Its attributes.
    #[serde(flatten)]
    pub attributes: ProbeAttributes,
    /// Why it could not be inspected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A shallow look at a folder from this process (`library.diagnostics`), so access problems of
/// the application itself are visible.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootProbe {
    /// The folder.
    pub path: String,
    /// Whether it is an existing directory.
    pub available: bool,
    /// Whether its entries could be listed.
    pub can_read_directory: bool,
    /// Number of entries.
    pub entry_count: usize,
    /// The first eight visible entries.
    pub sample_entries: Vec<ProbeEntry>,
    /// The folder's own attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<ProbeAttributes>,
    /// Why the folder could not be inspected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_error: Option<String>,
    /// Why its entries could not be listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The result of `library.state.set`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Saved {
    /// Always true.
    pub saved: bool,
}

/// What `kit.detail` says about loading a kit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitCompatibility {
    /// "Ready to load" or "Review import notes".
    pub summary: String,
    /// Always true: a kit loads with whatever is available.
    pub loadable: bool,
    /// Notes for the user.
    pub warnings: Vec<String>,
    /// Plug-ins the kit uses that are not installed.
    pub missing_plugins: Vec<String>,
}

/// The result of `kit.detail`: the kit's metadata fields plus these.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitDetail {
    /// The metadata of the kit or group (flattened into the result).
    #[serde(flatten)]
    pub metadata: Value,
    /// The pads.
    pub sounds: Vec<crate::kit::group::SoundSummary>,
    /// Whether it can be loaded, and what to expect.
    pub compatibility: KitCompatibility,
}

/// The result of `kit.read`: the kit as a group project the page inserts into the session.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitReadResponse {
    /// The kit package or group file.
    pub path: String,
    /// Its metadata.
    pub metadata: Value,
    /// The group project (see [`crate::kit::group`]), sample paths absolute.
    pub group_project: Value,
    /// The pads.
    pub sounds: Vec<crate::kit::group::SoundSummary>,
    /// Notes for the user.
    pub warnings: Vec<String>,
    /// Maschine groups: what was read and how each module was converted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<Value>,
}

/// The result of `kit.save`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum KitSaveResponse {
    /// The kit was written.
    Saved(crate::kit::SavedKit),
    /// Save As is needed: the kit has no package of its own in the user's kit folder.
    NeedsName {
        /// Always true.
        #[serde(rename = "needsName")]
        needs_name: bool,
        /// The path that was refused.
        path: String,
    },
    /// A kit of this name exists; ask before replacing it.
    Exists {
        /// Always true.
        exists: bool,
        /// The existing package.
        path: String,
        /// The requested name.
        name: String,
    },
}

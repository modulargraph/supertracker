//! The kit browser: indexed kits and Maschine groups by product (expansion), bank and sub-bank,
//! type and sub-type, and character, with expansion artwork.
//!
//! What Maschine's browser shows for a group comes from the group file's header. Headers are
//! read lazily in the background (64 KiB each) and remembered in `kit-headers.json` by file
//! size and date, so the library index itself stays small.

use super::{group_header_at, GroupHeader};
use crate::api::{page, KitsBrowseRequest};
use crate::nks::artwork::{ArtMatch, NksArtwork};
use crate::nks::catalog::{facet, subtype_facet, BrowseFacet, SUBTYPE_SEPARATOR};
use crate::util::{caseless_cmp, extension, file_name, fold, lock, natural_cmp, read_bounded, volume_name, write_atomic, Worker};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The product loose user kits are listed under.
pub const USER_PRODUCT: &str = "Saved kits";

/// An indexed kit as the catalogue sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Source {
    /// The kit package or group file.
    pub path: String,
    /// Display name.
    pub name: String,
    /// The pack the index inferred.
    pub pack: String,
    /// Name of the library root.
    pub root_name: String,
    /// The index's category (the kit type).
    pub category: String,
    /// Size of the file (or of a package's header).
    pub size: u64,
    /// Modification time.
    pub modified: f64,
    /// Tags.
    pub tags: Vec<String>,
    /// Audio preview.
    pub preview_path: Option<String>,
    /// Loaded pads.
    pub sound_count: Option<u32>,
    /// Why its header could not be read.
    pub metadata_error: Option<String>,
    /// Whether its library folder is reachable.
    pub available: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Cached {
    size: u64,
    modified: f64,
    header: Option<GroupHeader>,
}

/// The state of the background header reading (`status` of a browse result and the payload
/// of the `library.kits.progress` event).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct KitsStatus {
    /// Whether headers are being read.
    pub reading: bool,
    /// Headers read in this pass.
    pub read: usize,
    /// Headers to read in this pass.
    pub pending: usize,
    /// Changes when headers arrive: a page of results belongs to one revision.
    pub revision: u64,
}

/// How many kits match the search per tab.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct KitCounts {
    /// Library content.
    pub library: usize,
    /// The user's own kits and groups.
    pub user: usize,
}

/// The result of `library.kits.browse`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitsBrowseResponse {
    /// The page of kits.
    pub items: Vec<Value>,
    /// All matching kits.
    pub total: usize,
    /// The request's offset.
    pub offset: usize,
    /// The request's page size.
    pub limit: usize,
    /// Products (expansions).
    pub products: Vec<BrowseFacet>,
    /// Banks of the chosen product.
    pub banks: Vec<BrowseFacet>,
    /// Sub-banks of the chosen bank.
    pub sub_banks: Vec<BrowseFacet>,
    /// Types.
    pub types: Vec<BrowseFacet>,
    /// Sub-types.
    pub subtypes: Vec<BrowseFacet>,
    /// Characters (tags of the user's kits), most kits first.
    pub characters: Vec<BrowseFacet>,
    /// Totals of the two tabs.
    pub counts: KitCounts,
    /// Header reading.
    pub status: KitsStatus,
    /// Whether the library is being scanned.
    pub scanning: bool,
    /// Messages of the last library scan.
    pub errors: Vec<String>,
}

struct Entry<'a> {
    source: &'a Source,
    user: bool,
    product: String,
    bank: String,
    sub_bank: String,
    vendor: String,
    author: String,
    types: Vec<String>,
    subtypes: Vec<String>,
    characters: Vec<String>,
    search: String,
    folded_name: String,
}

#[derive(Default)]
struct State {
    headers: HashMap<String, Cached>,
    loaded: bool,
    reading: bool,
    revision: u64,
    pending: usize,
    read: usize,
}

type Events = Arc<dyn Fn(&str, Value) + Send + Sync>;

struct Shared {
    cache_path: Option<PathBuf>,
    state: Mutex<State>,
    events: Mutex<Option<Events>>,
}

/// The kit catalogue. Owns the worker thread that reads group headers.
pub struct KitCatalog {
    shared: Arc<Shared>,
    artwork: Arc<NksArtwork>,
    worker: Worker,
}

impl Shared {
    fn status(&self) -> KitsStatus {
        let state = lock(&self.state);
        KitsStatus { reading: state.reading, read: state.read, pending: state.pending, revision: state.revision }
    }

    fn emit(&self) {
        let sink = lock(&self.events).clone();
        if let Some(sink) = sink {
            sink("library.kits.progress", serde_json::to_value(self.status()).unwrap_or(Value::Null));
        }
    }
}

impl KitCatalog {
    /// A catalogue that remembers group headers in `cache_directory` (or not at all).
    pub fn new(cache_directory: Option<&Path>, artwork: Arc<NksArtwork>) -> KitCatalog {
        KitCatalog {
            shared: Arc::new(Shared { cache_path: cache_directory.map(|folder| folder.join("kit-headers.json")), state: Mutex::new(State::default()), events: Mutex::new(None) }),
            artwork,
            worker: Worker::new("st-library-kit-headers"),
        }
    }

    /// Where `library.kits.progress` events go.
    pub fn set_event_sink(&self, sink: Events) {
        *lock(&self.shared.events) = Some(sink);
    }

    /// The state of header reading.
    pub fn status(&self) -> KitsStatus {
        self.shared.status()
    }

    /// Waits until the headers queued so far have been read.
    pub fn wait_idle(&self) {
        self.worker.wait_idle();
    }

    fn load_if_needed(&self) {
        if lock(&self.shared.state).loaded {
            return;
        }
        let restored: HashMap<String, Cached> =
            self.shared.cache_path.as_ref().and_then(|path| read_bounded(path, 64 * 1024 * 1024)).and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default();
        let mut state = lock(&self.shared.state);
        if !state.loaded {
            state.headers = restored;
            state.loaded = true;
        }
    }

    /// Cached headers of current files; unknown or changed groups are queued for one background read.
    fn headers(&self, sources: &[Source]) -> HashMap<String, GroupHeader> {
        self.load_if_needed();
        let mut result = HashMap::new();
        let mut missing: Vec<(String, u64, f64)> = Vec::new();
        {
            let state = lock(&self.shared.state);
            for source in sources.iter().filter(|source| extension(&source.path) == "mxgrp") {
                match state.headers.get(&source.path) {
                    Some(cached) if cached.size == source.size && crate::util::same_time(cached.modified, source.modified) => {
                        if let Some(header) = &cached.header {
                            result.insert(source.path.clone(), header.clone());
                        }
                    }
                    _ if source.available => missing.push((source.path.clone(), source.size, source.modified)),
                    _ => {}
                }
            }
        }
        if !missing.is_empty() {
            self.read(missing);
        }
        result
    }

    fn read(&self, sources: Vec<(String, u64, f64)>) {
        {
            let mut state = lock(&self.shared.state);
            if state.reading {
                return;
            }
            state.reading = true;
            state.pending = sources.len();
            state.read = 0;
        }
        let shared = self.shared.clone();
        self.worker.submit(move || {
            let mut last_emit = Instant::now();
            for (index, (path, size, modified)) in sources.iter().enumerate() {
                let header = group_header_at(path);
                {
                    let mut state = lock(&shared.state);
                    state.headers.insert(path.clone(), Cached { size: *size, modified: *modified, header });
                    state.read = index + 1;
                }
                if index % 64 == 63 {
                    std::thread::sleep(Duration::from_millis(2)); // Leave disk bandwidth for playback.
                }
                if last_emit.elapsed() > Duration::from_millis(800) {
                    last_emit = Instant::now();
                    lock(&shared.state).revision += 1;
                    shared.emit();
                }
            }
            let snapshot = {
                let mut state = lock(&shared.state);
                state.reading = false;
                state.pending = 0;
                state.revision += 1;
                state.headers.clone()
            };
            if let Some(path) = &shared.cache_path {
                let _ = write_atomic(path, &serde_json::to_vec(&snapshot).unwrap_or_default());
            }
            shared.emit();
        });
    }

    fn entry<'a>(&self, source: &'a Source, header: Option<&GroupHeader>) -> Entry<'a> {
        let own = extension(&source.path) == "stkit";
        // Library content carries an NI bank chain; user groups and SuperTracker kits do not.
        let user = own || header.map(|header| header.chain.is_empty()).unwrap_or_else(|| !source.path.to_lowercase().replace('\\', "/").contains("/native access content/"));
        let (mut product, mut bank, mut sub_bank, mut vendor, mut author) = (source.pack.clone(), String::new(), String::new(), String::new(), String::new());
        match header.filter(|header| !header.chain.is_empty()) {
            Some(header) => {
                product = header.chain[0].clone();
                bank = header.chain.get(1).cloned().unwrap_or_default();
                sub_bank = header.chain.get(2).cloned().unwrap_or_default();
                vendor = header.vendor.clone();
                author = header.author.clone();
            }
            None if user => {
                // Sub-folders of a kit folder (and a kit's own expansion field) act as products; loose kits share one.
                if source.pack == source.root_name || ["kits", "groups"].contains(&source.pack.to_lowercase().as_str()) {
                    product = USER_PRODUCT.to_string();
                }
                author = header.map(|header| header.author.clone()).unwrap_or_default();
            }
            None => {}
        }
        let (types, subtypes, characters): (Vec<String>, Vec<String>, Vec<String>) = if let Some(header) = header {
            let mut types: Vec<String> = header.types.iter().map(|path| path[0].clone()).collect::<HashSet<_>>().into_iter().collect();
            types.sort();
            let mut subtypes: Vec<String> = header.types.iter().filter(|path| path.len() > 1).map(|path| path[..2].join(SUBTYPE_SEPARATOR)).collect::<HashSet<_>>().into_iter().collect();
            subtypes.sort();
            (types, subtypes, Vec::new())
        } else if own {
            // "Other" is the inferred placeholder, not a kit tag.
            let kind = if source.category.is_empty() || source.category == "Kit" { "Kits".to_string() } else { source.category.clone() };
            (vec![kind], Vec::new(), source.tags.iter().filter(|tag| *tag != "Other").cloned().collect())
        } else {
            (vec!["Kits".to_string()], Vec::new(), Vec::new())
        };
        let mut parts: Vec<String> = vec![source.name.clone(), product.clone(), bank.clone(), sub_bank.clone(), author.clone()];
        parts.extend(types.iter().cloned());
        parts.extend(subtypes.iter().map(|subtype| subtype.replace(SUBTYPE_SEPARATOR, " ")));
        parts.extend(characters.iter().cloned());
        Entry { source, user, product, bank, sub_bank, vendor, author, types, subtypes, characters, search: fold(&parts.join(" ")), folded_name: fold(&source.name) }
    }

    fn art(&self, product: &str, vendor: &str, bank: &str, user: bool) -> ArtMatch {
        if product == USER_PRODUCT || (vendor.is_empty() && !product.is_empty() && user) {
            ArtMatch::default()
        } else {
            self.artwork.find(product, if vendor.is_empty() { "Native Instruments" } else { vendor }, bank)
        }
    }

    /// Browses `sources` (the kits of the library index).
    pub fn browse(&self, request: &KitsBrowseRequest, sources: &[Source], favorites: &HashSet<String>) -> KitsBrowseResponse {
        let known = self.headers(sources);
        let entries: Vec<Entry> = sources.iter().map(|source| self.entry(source, known.get(&source.path))).collect();
        let query: String = request.query.chars().take(256).collect();
        let folded = fold(&query);
        let words: Vec<&str> = folded.split_whitespace().collect();
        let product = request.product.as_deref();
        let bank = product.and(request.bank.as_deref());
        let sub_bank = bank.and(request.sub_bank.as_deref());
        let set = |values: &[String]| -> HashSet<String> { values.iter().take(128).cloned().collect() };
        let (types, subtypes, characters) = (set(&request.types), set(&request.subtypes), set(&request.characters));
        let (offset, limit) = page(request.offset, request.limit);
        let mut counts = KitCounts::default();
        let mut products: HashMap<String, (usize, String)> = HashMap::new();
        let (mut banks, mut sub_banks): (HashMap<String, usize>, HashMap<String, usize>) = Default::default();
        let (mut type_counts, mut subtype_counts, mut character_counts): (HashMap<String, usize>, HashMap<String, usize>, HashMap<String, usize>) = Default::default();
        let bump = |counts: &mut HashMap<String, usize>, key: &str| *counts.entry(key.to_string()).or_insert(0) += 1;
        let mut matches: Vec<(&Entry, u32)> = Vec::new();
        for entry in &entries {
            if (request.favorites && !favorites.contains(&entry.source.path)) || !words.iter().all(|word| entry.search.contains(word)) {
                continue;
            }
            if entry.user {
                counts.user += 1;
            } else {
                counts.library += 1;
            }
            if entry.user != request.user {
                continue;
            }
            let p = product.is_none_or(|product| entry.product == product);
            let b = bank.is_none_or(|bank| entry.bank == bank) && sub_bank.is_none_or(|sub_bank| entry.sub_bank == sub_bank);
            let t = types.is_empty() || entry.types.iter().any(|value| types.contains(value));
            let s = subtypes.is_empty() || entry.subtypes.iter().any(|value| subtypes.contains(value));
            let c = characters.iter().all(|value| entry.characters.contains(value));
            if t && s && c {
                products.entry(entry.product.clone()).or_insert_with(|| (0, entry.vendor.clone())).0 += 1;
                if product.is_some() && p {
                    bump(&mut banks, &entry.bank);
                }
                if bank.is_some_and(|bank| entry.bank == bank) && p && !entry.sub_bank.is_empty() {
                    bump(&mut sub_banks, &entry.sub_bank);
                }
            }
            if p && b && c {
                for value in &entry.types {
                    bump(&mut type_counts, value);
                }
            }
            if p && b && t && c {
                for value in &entry.subtypes {
                    if types.is_empty() || types.iter().any(|kind| value.starts_with(&format!("{kind}{SUBTYPE_SEPARATOR}"))) {
                        bump(&mut subtype_counts, value);
                    }
                }
            }
            if p && b && t && s {
                for value in &entry.characters {
                    bump(&mut character_counts, value);
                }
            }
            if !(p && b && t && s && c) {
                continue;
            }
            matches.push((entry, words.iter().map(|word| if entry.folded_name.starts_with(word) { 3 } else if entry.folded_name.contains(word) { 2 } else { 0 }).sum()));
        }
        matches.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| if product.is_none() { caseless_cmp(&left.0.product, &right.0.product) } else { std::cmp::Ordering::Equal })
                .then_with(|| natural_cmp(&left.0.folded_name, &right.0.folded_name))
                .then_with(|| left.0.source.path.cmp(&right.0.source.path))
        });
        let items = matches
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(entry, _)| {
                let source = entry.source;
                let ext = extension(&source.path);
                let mut value = serde_json::json!({
                    "path": source.path, "id": source.path, "kind": "kit", "name": source.name, "product": entry.product, "bank": entry.bank, "subBank": entry.sub_bank,
                    "vendor": entry.vendor, "author": entry.author, "types": entry.types, "subtypes": entry.subtypes, "characters": entry.characters, "tags": entry.characters,
                    "favorite": favorites.contains(&source.path), "extension": ext, "user": entry.user, "kitFormat": if ext == "stkit" { "supertracker-kit" } else { "maschine-group" },
                    "pack": entry.product, "available": source.available,
                });
                if let Some(preview) = &source.preview_path {
                    value["previewPath"] = Value::from(preview.as_str());
                }
                if let Some(count) = source.sound_count {
                    value["soundCount"] = Value::from(count);
                }
                if let Some(error) = &source.metadata_error {
                    value["metadataError"] = Value::from(error.as_str());
                }
                let art = self.art(&entry.product, &entry.vendor, &entry.bank, entry.user);
                if let Some(key) = art.bank.as_ref().or(art.product.as_ref()) {
                    value["art"] = Value::from(key.as_str());
                }
                if let Some(key) = &art.product {
                    value["productArt"] = Value::from(key.as_str());
                }
                if let Some(color) = &art.color {
                    value["color"] = Value::from(color.as_str());
                }
                if !source.available {
                    if let Some(volume) = volume_name(&source.path) {
                        value["volume"] = Value::from(volume);
                    }
                }
                value
            })
            .collect();
        let mut product_names: Vec<&String> = products.keys().collect();
        product_names.sort_by(|left, right| caseless_cmp(left, right));
        let product_list: Vec<BrowseFacet> = product_names
            .into_iter()
            .map(|name| {
                let (count, vendor) = &products[name];
                let art = self.art(name, vendor, "", request.user);
                BrowseFacet { name: name.clone(), count: *count, vendor: Some(vendor.clone()), art: art.product, color: art.color, ..Default::default() }
            })
            .collect();
        let product_vendor = product.and_then(|name| products.get(name)).map(|value| value.1.clone()).unwrap_or_default();
        let bank_list = facet(&banks, false)
            .into_iter()
            .map(|mut value| {
                if let Some(product) = product.filter(|_| !value.name.is_empty()) {
                    let art = self.art(product, &product_vendor, &value.name, request.user);
                    value.art = art.bank;
                    value.color = art.color;
                }
                value
            })
            .collect();
        KitsBrowseResponse {
            items,
            total: matches.len(),
            offset,
            limit,
            products: product_list,
            banks: bank_list,
            sub_banks: facet(&sub_banks, false),
            types: facet(&type_counts, false),
            subtypes: subtype_facet(&subtype_counts),
            characters: facet(&character_counts, true),
            counts,
            status: self.status(),
            scanning: false,
            errors: Vec::new(),
        }
    }
}

/// The name of the root a kit path was indexed under, as the catalogue compares it with packs.
pub(crate) fn root_display_name(root: &str, names: &HashMap<String, String>) -> String {
    names.get(root).cloned().unwrap_or_else(|| file_name(root).to_string())
}

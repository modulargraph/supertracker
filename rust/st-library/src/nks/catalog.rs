//! The NKS preset browser: product, bank, type, sub-type and character browsing over every
//! installed preset, in the manner of Maschine and Komplete Kontrol.
//!
//! Presets are found in the content folders Native Instruments products register (macOS; see
//! [`discover_registrations`]) and among the NKS files the sound library indexed. Only the
//! `NISI` and `PLID` chunks are read; plug-in state is never touched while indexing. The index
//! is cached in `nks-catalog.json`; presets on folders that are not reachable are kept so
//! reconnecting a drive restores them without reading everything again.

use super::artwork::NksArtwork;
use super::metadata::catalog_summary;
use crate::api::{page, NksBrowseRequest};
use crate::util::{caseless_cmp, extension, file_name, fold, is_inside, is_readable_file, lock, modified_seconds, normalized, parent, read_bounded, volume_name, write_atomic, Worker};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Separates a type from its sub-type in sub-type keys ("Bass<US>Sub Bass").
pub const SUBTYPE_SEPARATOR: &str = "\u{1f}";

/// One preset of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    path: String,
    size: u64,
    modified: f64,
    name: String,
    vendor: String,
    product: String,
    bank: String,
    sub_bank: String,
    effect: bool,
    types: Vec<String>,
    subtypes: Vec<String>,
    characters: Vec<String>,
    author: String,
    comment: String,
    preview: Option<String>,
    format: String,
    has_controls: bool,
    #[serde(skip)]
    search: String,
    #[serde(skip)]
    folded_name: String,
}

impl Entry {
    fn prepare(&mut self) {
        self.folded_name = fold(&self.name);
        let mut parts: Vec<String> = vec![self.name.clone(), self.product.clone(), self.vendor.clone(), self.bank.clone(), self.sub_bank.clone(), self.author.clone(), self.comment.clone()];
        parts.extend(self.types.iter().cloned());
        parts.extend(self.subtypes.iter().map(|subtype| subtype.replace(SUBTYPE_SEPARATOR, " ")));
        parts.extend(self.characters.iter().cloned());
        self.search = fold(&parts.join(" "));
    }
}

/// A product's content folder as its installer registered it.
#[derive(Debug, Clone, PartialEq)]
pub struct Registration {
    /// Product name.
    pub name: String,
    /// Vendor.
    pub company: String,
    /// The folder that holds the product's content.
    pub content_dir: String,
    /// Whether the product is an effect, when its registration says.
    pub effect: Option<bool>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: Vec<Entry>,
}

/// The state of the catalogue (`library.nks.status`, the `library.nks.progress` event and
/// `status` of a browse result).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct NksStatus {
    /// Whether presets are being read.
    pub indexing: bool,
    /// Presets read in this pass.
    pub done: usize,
    /// Presets to read in this pass.
    pub total: usize,
    /// Presets in the catalogue.
    pub count: usize,
    /// Changes with the catalogue's content: a page of results belongs to one revision.
    pub revision: u64,
    /// Folders scanned.
    pub roots: usize,
    /// What could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A value of a browse facet.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct BrowseFacet {
    /// The value (for sub-types "Type<US>Sub-type").
    pub name: String,
    /// Presets or kits with it.
    pub count: usize,
    /// Products: the prevailing vendor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Artwork key for `library.nks.artwork`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// Background colour, `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Sub-types: the parent type.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Sub-types: the name without the parent type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// How many presets match the search per tab.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct KindCounts {
    /// Instrument presets.
    pub instrument: usize,
    /// Effect presets.
    pub effect: usize,
}

/// The result of `library.nks.browse`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NksBrowseResponse {
    /// The page of presets.
    pub items: Vec<Value>,
    /// All matching presets.
    pub total: usize,
    /// The request's offset.
    pub offset: usize,
    /// The request's page size.
    pub limit: usize,
    /// Products (with artwork and colour).
    pub products: Vec<BrowseFacet>,
    /// Banks of the chosen product.
    pub banks: Vec<BrowseFacet>,
    /// Sub-banks of the chosen bank.
    pub sub_banks: Vec<BrowseFacet>,
    /// Vendors, most presets first.
    pub vendors: Vec<BrowseFacet>,
    /// Types.
    pub types: Vec<BrowseFacet>,
    /// Sub-types of the chosen types.
    pub subtypes: Vec<BrowseFacet>,
    /// Characters, most presets first.
    pub characters: Vec<BrowseFacet>,
    /// Totals of the two tabs.
    pub counts: KindCounts,
    /// The catalogue's state; `revision` is the one these results belong to.
    pub status: NksStatus,
}

/// Preset files and library folders the sound library contributes.
#[derive(Debug, Clone, Default)]
pub struct Extra {
    /// NKS files the library indexed (also on folders that are offline now).
    pub presets: Vec<String>,
    /// The library's folders; used only to keep presets while a folder is unavailable.
    pub roots: Vec<String>,
}

type Events = Arc<dyn Fn(&str, Value) + Send + Sync>;
type ExtraProvider = Arc<dyn Fn() -> Extra + Send + Sync>;

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    revision: u64,
    loaded: bool,
    activated: bool,
    indexing: bool,
    refresh_requested: bool,
    done: usize,
    total: usize,
    last_error: Option<String>,
    last_root_count: usize,
}

struct Shared {
    cache_path: PathBuf,
    discover_system_content: bool,
    fixed_roots: Vec<String>,
    state: Mutex<State>,
    load_lock: Mutex<()>,
    events: Mutex<Option<Events>>,
    extra: Mutex<Option<ExtraProvider>>,
    artwork: Arc<NksArtwork>,
    pause: Duration,
}

/// The preset catalogue. Owns a worker thread that reads presets in the background.
pub struct NksCatalog {
    shared: Arc<Shared>,
    worker: Worker,
}

/// Some vendors ship internal selection markers ("_ SEL LEAD (ROUND 4)") as NKS modes.
fn visible_character(value: &str) -> bool {
    !value.starts_with('_') && !value.trim().is_empty()
}

pub(crate) fn facet(counts: &HashMap<String, usize>, by_count: bool) -> Vec<BrowseFacet> {
    let mut values: Vec<(&String, &usize)> = counts.iter().collect();
    values.sort_by(|left, right| if by_count && left.1 != right.1 { right.1.cmp(left.1) } else { caseless_cmp(left.0, right.0) });
    values.into_iter().take(1000).map(|(name, count)| BrowseFacet { name: name.clone(), count: *count, ..Default::default() }).collect()
}

pub(crate) fn subtype_facet(counts: &HashMap<String, usize>) -> Vec<BrowseFacet> {
    facet(counts, false)
        .into_iter()
        .map(|mut value| {
            let mut parts = value.name.split(SUBTYPE_SEPARATOR);
            value.parent = Some(parts.next().unwrap_or("").to_string());
            value.label = Some(value.name.rsplit(SUBTYPE_SEPARATOR).next().unwrap_or("").to_string());
            value
        })
        .collect()
}

impl NksCatalog {
    /// A catalogue cached in `cache_directory`. `roots` are always scanned; with
    /// `discover_system_content` the registered content folders of installed products are too.
    pub fn new(cache_directory: &Path, discover_system_content: bool, roots: &[String], artwork: Arc<NksArtwork>) -> NksCatalog {
        NksCatalog {
            shared: Arc::new(Shared {
                cache_path: cache_directory.join("nks-catalog.json"),
                discover_system_content,
                fixed_roots: roots.iter().map(|root| normalized(root)).collect(),
                state: Mutex::new(State::default()),
                load_lock: Mutex::new(()),
                events: Mutex::new(None),
                extra: Mutex::new(None),
                artwork,
                pause: Duration::from_millis(1),
            }),
            worker: Worker::new("st-library-nks"),
        }
    }

    /// Where `library.nks.progress` events go.
    pub fn set_event_sink(&self, sink: Events) {
        *lock(&self.shared.events) = Some(sink);
    }

    /// Where the catalogue asks for the library's NKS files and folders at each refresh.
    pub fn set_extra_provider(&self, provider: ExtraProvider) {
        *lock(&self.shared.extra) = Some(provider);
    }

    /// The artwork index.
    pub fn artwork(&self) -> &Arc<NksArtwork> {
        &self.shared.artwork
    }

    /// Whether the catalogue has been used (browsed or refreshed) since start.
    pub fn is_active(&self) -> bool {
        lock(&self.shared.state).activated
    }

    /// The catalogue's state.
    pub fn status(&self) -> NksStatus {
        self.shared.status()
    }

    /// Starts an incremental refresh in the background. A request during a refresh runs once
    /// more afterwards.
    pub fn refresh(&self) {
        let start = {
            let mut state = lock(&self.shared.state);
            state.activated = true;
            if state.indexing {
                state.refresh_requested = true;
                false
            } else {
                state.indexing = true;
                state.done = 0;
                state.total = 0;
                true
            }
        };
        if !start {
            return;
        }
        self.shared.emit_status();
        let shared = self.shared.clone();
        self.worker.submit(move || {
            while shared.run_refresh() {
                let mut state = lock(&shared.state);
                state.indexing = true;
                state.done = 0;
                state.total = 0;
            }
        });
    }

    /// Refreshes and returns when the catalogue is up to date.
    pub fn refresh_now(&self) {
        self.refresh();
        self.worker.wait_idle();
    }

    /// Waits for a running refresh.
    pub fn wait_idle(&self) {
        self.worker.wait_idle();
    }

    /// Browses the catalogue. The first call starts a refresh in the background.
    pub fn browse(&self, request: &NksBrowseRequest, favorites: &HashSet<String>) -> NksBrowseResponse {
        self.shared.load_cache_if_needed();
        if !self.is_active() {
            self.refresh();
        }
        self.shared.browse(request, favorites)
    }
}

impl Shared {
    fn status(&self) -> NksStatus {
        let state = lock(&self.state);
        NksStatus { indexing: state.indexing, done: state.done, total: state.total, count: state.entries.len(), revision: state.revision, roots: state.last_root_count, error: state.last_error.clone() }
    }

    fn emit_status(&self) {
        let sink = lock(&self.events).clone();
        if let Some(sink) = sink {
            sink("library.nks.progress", serde_json::to_value(self.status()).unwrap_or(Value::Null));
        }
    }

    fn load_cache_if_needed(&self) {
        let _guard = lock(&self.load_lock);
        if lock(&self.state).loaded {
            return;
        }
        let mut decoded = HashMap::new();
        let file = read_bounded(&self.cache_path, 256 * 1024 * 1024).and_then(|data| serde_json::from_slice::<CacheFile>(&data).ok());
        if let Some(file) = file.filter(|file| file.version == 1) {
            for mut entry in file.entries {
                entry.prepare();
                decoded.insert(entry.path.clone(), entry);
            }
        }
        let mut state = lock(&self.state);
        if !state.loaded {
            state.entries = decoded;
            state.loaded = true;
            state.revision += 1;
        }
    }

    fn persist(&self) {
        let snapshot: Vec<Entry> = lock(&self.state).entries.values().cloned().collect();
        let data = serde_json::to_vec(&CacheFile { version: 1, entries: snapshot }).unwrap_or_default();
        if let Err(error) = write_atomic(&self.cache_path, &data) {
            lock(&self.state).last_error = Some(format!("Could not save the NKS catalogue cache: {error}"));
        }
    }

    /// One refresh pass. Returns whether another was requested meanwhile.
    fn run_refresh(&self) -> bool {
        self.load_cache_if_needed();
        let registrations = if self.discover_system_content { system_registrations() } else { Vec::new() };
        let mut roots = self.fixed_roots.clone();
        if self.discover_system_content {
            roots.extend(registrations.iter().map(|registration| registration.content_dir.clone()));
            if let Some(home) = crate::util::home_dir() {
                roots.push(home.join("Documents/Native Instruments/User Content").to_string_lossy().into_owned());
            }
        }
        // Nested roots are scanned once.
        let mut unique: Vec<String> = roots.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
        unique.sort_by_key(|root| root.len());
        let mut scan_roots: Vec<String> = Vec::new();
        for root in unique {
            if !scan_roots.iter().any(|known| is_inside(&root, known)) {
                scan_roots.push(root);
            }
        }
        let mut found: HashMap<String, (u64, f64)> = HashMap::new();
        let mut unavailable: Vec<String> = Vec::new();
        let mut scan_errors: Vec<String> = Vec::new();
        let mut record_unavailable = |root: &str, message: String| {
            if !unavailable.iter().any(|known| known == root) {
                unavailable.push(root.to_string());
            }
            if scan_errors.len() < 16 {
                scan_errors.push(message);
            }
        };
        // Library files arrive through the extra provider, including cached paths on disconnected
        // drives. Their folders are not scanned again here, and their presets are kept while absent.
        let extra = lock(&self.extra).clone().map(|provider| provider()).unwrap_or_default();
        for root in &extra.roots {
            if fs::read_dir(root).is_err() {
                record_unavailable(root, format!("Library folder unavailable: {root}"));
            }
        }
        for root in &scan_roots {
            if !crate::util::is_directory(root) {
                record_unavailable(root, format!("NKS folder unavailable: {root}"));
                continue;
            }
            let mut stack = vec![PathBuf::from(root)];
            let mut visited = 0usize;
            'walk: while let Some(folder) = stack.pop() {
                let entries = match fs::read_dir(&folder) {
                    Ok(entries) => entries,
                    Err(error) => {
                        record_unavailable(root, format!("Could not read {}: {}", folder.display(), crate::error::io_reason(&error)));
                        continue;
                    }
                };
                for entry in entries.flatten() {
                    visited += 1;
                    if visited > 600_000 {
                        record_unavailable(root, format!("NKS folder scan limit reached: {root}. Previously indexed presets have been retained."));
                        break 'walk;
                    }
                    if visited % 512 == 0 && !self.pause.is_zero() {
                        std::thread::sleep(self.pause * 2);
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    let Ok(kind) = entry.file_type() else {
                        record_unavailable(root, format!("Could not read NKS folder entry: {}", entry.path().display()));
                        continue;
                    };
                    if kind.is_symlink() {
                        continue;
                    }
                    let path = entry.path().to_string_lossy().into_owned();
                    let ext = extension(&path);
                    if kind.is_dir() {
                        if !["app", "component", "vst", "vst3", "bundle", "framework", "aaxplugin"].contains(&ext.as_str()) && name.to_lowercase() != "samples" {
                            stack.push(entry.path());
                        }
                        continue;
                    }
                    if kind.is_file() && (ext == "nksf" || ext == "nksfx") {
                        if let Ok(metadata) = entry.metadata() {
                            if metadata.len() > 0 {
                                found.insert(path, (metadata.len(), modified_seconds(&metadata)));
                            }
                        }
                    }
                }
            }
        }
        for path in &extra.presets {
            if found.contains_key(path) || !["nksf", "nksfx"].contains(&extension(path).as_str()) {
                continue;
            }
            if let Ok(metadata) = fs::metadata(path) {
                found.insert(path.clone(), (metadata.len(), modified_seconds(&metadata)));
            }
        }
        let mut previews: HashMap<String, HashSet<String>> = HashMap::new();
        let mut preview_path = |path: &str| -> Option<String> {
            let directory = parent(path).to_string();
            let names = previews.entry(directory.clone()).or_insert_with(|| {
                fs::read_dir(Path::new(&directory).join(".previews")).map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
            });
            let (file, stem) = (file_name(path), crate::util::stem(path));
            [format!("{file}.ogg"), format!("{stem}.ogg"), format!("{file}.wav"), format!("{stem}.wav")]
                .into_iter()
                .find(|name| names.contains(name))
                .map(|name| Path::new(&directory).join(".previews").join(name).to_string_lossy().into_owned())
        };
        let snapshot: HashMap<String, (u64, f64, Option<String>)> = lock(&self.state).entries.iter().map(|(path, entry)| (path.clone(), (entry.size, entry.modified, entry.preview.clone()))).collect();
        let mut pending: Vec<String> = Vec::new();
        let mut preview_updates: Vec<(String, Option<String>)> = Vec::new();
        for (path, signature) in &found {
            match snapshot.get(path) {
                Some((size, modified, preview)) if size == &signature.0 && crate::util::same_time(*modified, signature.1) => {
                    let current = preview_path(path);
                    if &current != preview {
                        preview_updates.push((path.clone(), current));
                    }
                }
                _ => pending.push(path.clone()),
            }
        }
        pending.sort();
        {
            let mut state = lock(&self.state);
            let changed = !preview_updates.is_empty();
            for (path, preview) in preview_updates {
                if let Some(entry) = state.entries.get_mut(&path) {
                    entry.preview = preview;
                }
            }
            if changed {
                state.revision += 1;
            }
            state.total = pending.len();
            state.last_root_count = scan_roots.len();
            state.last_error = (!scan_errors.is_empty()).then(|| scan_errors.join("\n"));
        }
        self.emit_status();
        let mut batch: Vec<Entry> = Vec::new();
        let mut last_commit = Instant::now();
        let commit = |batch: &mut Vec<Entry>, last_commit: &mut Instant| {
            if batch.is_empty() {
                return;
            }
            {
                let mut state = lock(&self.state);
                state.done += batch.len();
                for entry in batch.drain(..) {
                    state.entries.insert(entry.path.clone(), entry);
                }
                state.revision += 1;
            }
            *last_commit = Instant::now();
            self.emit_status();
        };
        for (index, path) in pending.iter().enumerate() {
            let signature = found[path];
            match make_entry(path, signature.0, signature.1, &registrations, preview_path(path)) {
                Some(entry) => batch.push(entry),
                None => lock(&self.state).done += 1,
            }
            if index % 128 == 127 && !self.pause.is_zero() {
                std::thread::sleep(self.pause);
            }
            if batch.len() >= 400 || last_commit.elapsed() > Duration::from_millis(500) {
                commit(&mut batch, &mut last_commit);
            }
        }
        commit(&mut batch, &mut last_commit);
        {
            // Presets on disconnected folders stay, so reconnecting restores them without reading again.
            let mut state = lock(&self.state);
            let stale: Vec<String> = state.entries.keys().filter(|path| !found.contains_key(*path) && !unavailable.iter().any(|root| is_inside(path, root))).cloned().collect();
            for path in &stale {
                state.entries.remove(path);
            }
            if !stale.is_empty() {
                state.revision += 1;
            }
        }
        self.persist();
        let again = {
            let mut state = lock(&self.state);
            state.indexing = false;
            std::mem::take(&mut state.refresh_requested)
        };
        self.emit_status();
        again
    }

    fn browse(&self, request: &NksBrowseRequest, favorites: &HashSet<String>) -> NksBrowseResponse {
        let (all, revision): (Vec<Entry>, u64) = {
            let state = lock(&self.state);
            (state.entries.values().cloned().collect(), state.revision)
        };
        let effect = request.kind == "effect";
        let query: String = request.query.chars().take(256).collect();
        let folded = fold(&query);
        let words: Vec<&str> = folded.split_whitespace().collect();
        let product = request.product.as_deref();
        let bank = request.bank.as_deref();
        let sub_bank = bank.and(request.sub_bank.as_deref());
        let vendor = request.vendor.as_deref();
        let set = |values: &[String]| -> HashSet<String> { values.iter().take(128).cloned().collect() };
        let (types, subtypes, characters) = (set(&request.types), set(&request.subtypes), set(&request.characters));
        let (offset, limit) = page(request.offset, request.limit);

        let mut kind_counts = KindCounts::default();
        let mut product_counts: HashMap<String, (usize, BTreeMap<String, usize>)> = HashMap::new();
        let (mut vendor_counts, mut sub_bank_counts, mut bank_counts): (HashMap<String, usize>, HashMap<String, usize>, HashMap<String, usize>) = Default::default();
        let (mut type_counts, mut subtype_counts, mut character_counts): (HashMap<String, usize>, HashMap<String, usize>, HashMap<String, usize>) = Default::default();
        let mut matches: Vec<(&Entry, u32)> = Vec::new();
        let bump = |counts: &mut HashMap<String, usize>, key: &str| *counts.entry(key.to_string()).or_insert(0) += 1;
        for entry in &all {
            if (request.favorites && !favorites.contains(&entry.path)) || !words.iter().all(|word| entry.search.contains(word)) {
                continue;
            }
            // Vendor narrows the product list; a chosen product keeps its own vendor.
            let vendor_matches = vendor.is_none_or(|vendor| entry.vendor == vendor);
            let p = product.is_none_or(|product| entry.product == product) && vendor_matches;
            let b = bank.is_none_or(|bank| entry.bank == bank) && sub_bank.is_none_or(|sub_bank| entry.sub_bank == sub_bank);
            let t = types.is_empty() || entry.types.iter().any(|value| types.contains(value));
            let s = subtypes.is_empty() || entry.subtypes.iter().any(|value| subtypes.contains(value));
            let c = characters.iter().all(|value| entry.characters.contains(value));
            // Tab totals follow only search and favorites; product and tags are per kind.
            if entry.effect {
                kind_counts.effect += 1;
            } else {
                kind_counts.instrument += 1;
            }
            if entry.effect != effect {
                continue;
            }
            if t && s && c {
                bump(&mut vendor_counts, &entry.vendor);
                if vendor_matches {
                    let counts = product_counts.entry(entry.product.clone()).or_default();
                    counts.0 += 1;
                    *counts.1.entry(entry.vendor.clone()).or_insert(0) += 1;
                }
                if product.is_some() && p {
                    bump(&mut bank_counts, &entry.bank);
                }
                if bank.is_some_and(|bank| entry.bank == bank) && p && !entry.sub_bank.is_empty() {
                    bump(&mut sub_bank_counts, &entry.sub_bank);
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
                for value in entry.characters.iter().filter(|value| visible_character(value)) {
                    bump(&mut character_counts, value);
                }
            }
            if !(p && b && t && s && c) {
                continue;
            }
            let score = words.iter().map(|word| if entry.folded_name.starts_with(word) { 3 } else if entry.folded_name.contains(word) { 2 } else { 0 }).sum();
            matches.push((entry, score));
        }
        matches.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.folded_name.cmp(&right.0.folded_name)).then_with(|| left.0.path.cmp(&right.0.path)));
        let items = matches
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(entry, _)| {
                let mut value = serde_json::json!({
                    "path": entry.path, "id": entry.path, "kind": "nks", "name": entry.name, "product": entry.product, "vendor": entry.vendor,
                    "bank": entry.bank, "subBank": entry.sub_bank, "effect": entry.effect, "types": entry.types, "subtypes": entry.subtypes,
                    "characters": entry.characters.iter().filter(|value| visible_character(value)).collect::<Vec<_>>(), "author": entry.author, "comment": entry.comment,
                    "format": entry.format, "hasControls": entry.has_controls, "favorite": favorites.contains(&entry.path), "extension": extension(&entry.path),
                });
                if let Some(preview) = &entry.preview {
                    value["previewPath"] = Value::from(preview.as_str());
                }
                let art = self.artwork.find(&entry.product, &entry.vendor, &entry.bank);
                if let Some(key) = art.bank.as_ref().or(art.product.as_ref()) {
                    value["art"] = Value::from(key.as_str());
                }
                if let Some(key) = &art.product {
                    value["productArt"] = Value::from(key.as_str());
                }
                if let Some(color) = &art.color {
                    value["color"] = Value::from(color.as_str());
                }
                // Only the bounded visible page is checked, never the whole catalogue per keystroke.
                if !is_readable_file(Path::new(&entry.path)) {
                    value["available"] = Value::Bool(false);
                    if let Some(volume) = volume_name(&entry.path) {
                        value["volume"] = Value::from(volume);
                    }
                }
                value
            })
            .collect();
        let mut product_names: Vec<&String> = product_counts.keys().collect();
        product_names.sort_by(|left, right| caseless_cmp(left, right));
        let products: Vec<BrowseFacet> = product_names
            .into_iter()
            .map(|name| {
                let (count, vendors) = &product_counts[name];
                // The vendor most presets name; ties go to the alphabetically first.
                let vendor = vendors.iter().max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0))).map(|(vendor, _)| vendor.clone()).unwrap_or_default();
                let art = self.artwork.find(name, &vendor, "");
                BrowseFacet { name: name.clone(), count: *count, vendor: Some(vendor), art: art.product, color: art.color, ..Default::default() }
            })
            .collect();
        let product_vendor = product.and_then(|name| products.iter().find(|value| value.name == name)).and_then(|value| value.vendor.clone()).unwrap_or_default();
        let banks = facet(&bank_counts, false)
            .into_iter()
            .map(|mut value| {
                if let Some(product) = product.filter(|_| !value.name.is_empty()) {
                    let art = self.artwork.find(product, &product_vendor, &value.name);
                    value.art = art.bank;
                    value.color = art.color;
                }
                value
            })
            .collect();
        vendor_counts.remove("");
        let mut status = self.status();
        // A refresh can publish while this page is sorted; the revision describes these entries.
        status.revision = revision;
        NksBrowseResponse {
            items,
            total: matches.len(),
            offset,
            limit,
            products,
            banks,
            sub_banks: facet(&sub_bank_counts, false),
            vendors: facet(&vendor_counts, true),
            types: facet(&type_counts, false),
            subtypes: subtype_facet(&subtype_counts),
            characters: facet(&character_counts, true),
            counts: kind_counts,
            status,
        }
    }
}

fn make_entry(path: &str, size: u64, modified: f64, registrations: &[Registration], preview: Option<String>) -> Option<Entry> {
    let summary = catalog_summary(path).ok()?;
    let registration = registrations.iter().filter(|registration| is_inside(path, &registration.content_dir)).max_by_key(|registration| registration.content_dir.len());
    let device = summary.device_type.to_uppercase();
    let effect = match device.as_str() {
        "FX" => true,
        "INST" => false,
        _ => registration.and_then(|registration| registration.effect).unwrap_or(extension(path) == "nksfx"),
    };
    // Vendors spell the same NKS type differently ("Piano/Keys", "Piano / Keys").
    let tidy = |value: &str| value.split('/').map(str::trim).collect::<Vec<_>>().join(" / ");
    let (mut types, mut subtypes): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for parts in summary.type_paths.iter().map(|path| path.iter().map(|part| tidy(part)).collect::<Vec<_>>()) {
        if let Some(first) = parts.first().filter(|first| !types.contains(first)) {
            types.push(first.clone());
        }
        if parts.len() > 1 {
            let key = format!("{}{SUBTYPE_SEPARATOR}{}", parts[0], parts[1]);
            if !subtypes.contains(&key) {
                subtypes.push(key);
            }
        }
    }
    let mut entry = Entry {
        path: path.to_string(),
        size,
        modified,
        name: summary.name,
        vendor: if summary.vendor.is_empty() { registration.map(|registration| registration.company.clone()).unwrap_or_default() } else { summary.vendor },
        product: if summary.product.is_empty() { registration.map(|registration| registration.name.clone()).unwrap_or_else(|| file_name(parent(path)).to_string()) } else { summary.product },
        bank: summary.bank,
        sub_bank: summary.sub_bank,
        effect,
        types,
        subtypes,
        characters: summary.characters,
        author: summary.author,
        comment: summary.comment,
        preview,
        format: summary.format,
        has_controls: summary.has_controls,
        search: String::new(),
        folded_name: String::new(),
    };
    entry.prepare();
    Some(entry)
}

// ---------------------------------------------------------------------------------------------
// Product registrations (macOS)

/// A classic Mac path ("Macintosh HD:Library:Massive X:") as a POSIX path. A path that is
/// already POSIX is returned as it is.
pub fn posix_path(from_hfs: &str, boot_volume: &str) -> Option<String> {
    if from_hfs.starts_with('/') && !from_hfs.starts_with("/:") {
        return (!from_hfs.contains(':')).then(|| from_hfs.to_string());
    }
    let mut parts: Vec<&str> = from_hfs.split(':').collect();
    while parts.last() == Some(&"") {
        parts.pop();
    }
    let volume = *parts.first().filter(|volume| !volume.is_empty())?;
    if parts.len() < 2 {
        return None;
    }
    let rest = parts[1..].join("/");
    Some(format!("{}/{rest}", if volume == "/" || volume == boot_volume { String::new() } else { format!("/Volumes/{volume}") }))
}

fn xml_value(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    let value = xml[start..end].trim().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    (!value.is_empty()).then_some(value)
}

/// The string value of a top-level key of a property list (XML or binary).
pub fn plist_string(data: &[u8], key: &str) -> Option<String> {
    if data.starts_with(b"bplist00") {
        return binary_plist_string(data, key);
    }
    let xml = std::str::from_utf8(data).ok()?;
    let after = &xml[xml.find(&format!("<key>{key}</key>"))? + key.len() + 11..];
    let trimmed = after.trim_start();
    let body = trimmed.strip_prefix("<string>")?;
    let value = &body[..body.find("</string>")?];
    Some(value.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}

fn binary_plist_string(data: &[u8], key: &str) -> Option<String> {
    let trailer = data.get(data.len().checked_sub(32)?..)?;
    let (offset_size, reference_size) = (trailer[6] as usize, trailer[7] as usize);
    let word = |bytes: &[u8]| bytes.iter().fold(0u64, |value, byte| value << 8 | u64::from(*byte)) as usize;
    let (count, top, table) = (word(&trailer[8..16]), word(&trailer[16..24]), word(&trailer[24..32]));
    if offset_size == 0 || offset_size > 8 || reference_size == 0 || reference_size > 8 || count > 1_000_000 {
        return None;
    }
    let offset = |index: usize| -> Option<usize> {
        if index >= count {
            return None;
        }
        Some(word(data.get(table + index * offset_size..table + (index + 1) * offset_size)?))
    };
    // The length of an object: the marker's low nibble, or an integer object that follows it.
    let sized = |at: usize| -> Option<(usize, usize)> {
        let low = (*data.get(at)? & 0x0f) as usize;
        if low != 0x0f {
            return Some((low, at + 1));
        }
        let marker = *data.get(at + 1)?;
        if marker & 0xf0 != 0x10 {
            return None;
        }
        let width = 1usize << (marker & 0x0f);
        Some((word(data.get(at + 2..at + 2 + width)?), at + 2 + width))
    };
    let string = |index: usize| -> Option<String> {
        let at = offset(index)?;
        let (length, start) = sized(at)?;
        match data.get(at)? & 0xf0 {
            0x50 => String::from_utf8(data.get(start..start + length)?.to_vec()).ok(),
            0x60 => {
                let raw = data.get(start..start + length * 2)?;
                Some(String::from_utf16_lossy(&raw.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect::<Vec<_>>()))
            }
            _ => None,
        }
    };
    let at = offset(top)?;
    if data.get(at)? & 0xf0 != 0xd0 {
        return None;
    }
    let (pairs, start) = sized(at)?;
    let reference = |position: usize| -> Option<usize> { Some(word(data.get(start + position * reference_size..start + (position + 1) * reference_size)?)) };
    (0..pairs).find(|index| reference(*index).and_then(string).as_deref() == Some(key)).and_then(|index| string(reference(pairs + index)?))
}

/// Reads product registrations from `preference_directories`: each
/// `com.native-instruments.<Product>.plist` names a content folder (`ContentDir`, a classic Mac
/// path); the matching Service Center XML names the product, its vendor and whether it is an effect.
pub fn discover_registrations(preference_directories: &[PathBuf], service_center: &Path, boot_volume: &str) -> Vec<Registration> {
    let mut result: BTreeMap<String, Registration> = BTreeMap::new();
    for directory in preference_directories {
        let Ok(entries) = fs::read_dir(directory) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(key) = name.strip_prefix("com.native-instruments.").and_then(|rest| rest.strip_suffix(".plist")) else { continue };
            let Some(path) = read_bounded(&entry.path(), 1_048_575).and_then(|data| plist_string(&data, "ContentDir")).and_then(|content| posix_path(&content, boot_volume)) else { continue };
            let (mut product, mut company, mut effect) = (key.to_string(), String::new(), None);
            if let Some(xml) = read_bounded(&service_center.join(format!("{key}.xml")), 4 * 1024 * 1024).and_then(|data| String::from_utf8(data).ok()) {
                product = xml_value(&xml, "Name").unwrap_or_else(|| key.to_string());
                company = xml_value(&xml, "Company").unwrap_or_default();
                if let Some(at) = xml.find("<PluginID fx=\"") {
                    let rest = &xml[at + 14..];
                    effect = if rest.starts_with("true\"") { Some(true) } else if rest.starts_with("false\"") { Some(false) } else { None };
                }
            }
            if company.is_empty() && product == key {
                if let Some((prefix, _)) = key.split_once('-') {
                    company = prefix.to_string();
                }
            }
            let content_dir = normalized(&path);
            result.insert(content_dir.clone(), Registration { name: product, company, content_dir, effect });
        }
    }
    result.into_values().collect()
}

/// The registrations of the products installed on this machine. Empty where the platform has
/// no such registry this crate reads (everything but macOS).
pub fn system_registrations() -> Vec<Registration> {
    #[cfg(target_os = "macos")]
    {
        let mut directories = vec![PathBuf::from("/Library/Preferences")];
        if let Some(home) = crate::util::home_dir() {
            directories.push(home.join("Library/Preferences"));
        }
        // The boot volume is listed in /Volumes as a link to "/".
        let boot_volume = fs::read_dir("/Volumes")
            .ok()
            .and_then(|entries| entries.flatten().find(|entry| fs::read_link(entry.path()).is_ok_and(|target| target == Path::new("/"))).map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Macintosh HD".to_string());
        discover_registrations(&directories, Path::new("/Library/Application Support/Native Instruments/Service Center"), &boot_volume)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

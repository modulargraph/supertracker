//! The [`Library`] service: what the bridge commands `library.*` and `kit.*` call.

use crate::api::*;
use crate::error::{io_reason, Error, Result};
use crate::index::{self, Cache, Field, Item, Kind, MetadataFile, Override, Record, Root, Tables};
use crate::infer::{clean_pack, infer_path, nks_category};
use crate::kit::catalog::{KitCatalog, KitsBrowseResponse, Source};
use crate::kit::{self, KitMetadata};
use crate::nks::artwork::NksArtwork;
use crate::nks::catalog::{Extra, NksBrowseResponse, NksCatalog, NksStatus};
use crate::nks::{NksHostPreset, NksMetadata};
use crate::search::{self, SearchCache, Snapshot};
use crate::util::{self, extension, is_directory, is_inside, lock, modified_seconds, normalized, Worker};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// Receives the service's events: the event name (`library.progress`, `library.results`,
/// `library.metadata`, `library.favorite`, `library.error`, `library.kits.progress`,
/// `library.nks.progress`) and its payload. Called from the service's worker threads.
pub type EventSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// How a [`Library`] is set up.
#[derive(Debug, Clone)]
pub struct LibraryConfig {
    /// Where the library keeps its files (index cache, roots, favorites, labels, browser state,
    /// catalogue caches). Created when missing.
    pub support_dir: PathBuf,
    /// The folder `kit.save` writes to and that is always a library root. `None`: no kit folder.
    pub user_kits_dir: Option<PathBuf>,
    /// `None`: start from the platform's standard content locations and installed NKS content.
    /// `Some`: start from exactly these folders and never look at system content (tests, headless use).
    pub default_roots: Option<Vec<PathBuf>>,
    /// Load the index cache before [`Library::open`] returns instead of in the background.
    pub load_cache_synchronously: bool,
    /// Pause after every 256 entries of a scan, so a bulk scan leaves disk and CPU to playback.
    pub scan_pause: Duration,
    /// Pause between two header reads of the duration worker.
    pub metadata_pause: Duration,
    /// How often library folders are checked for having appeared or disappeared (external
    /// drives). `None`: only when a search or browse asks.
    pub watch_interval: Option<Duration>,
    /// How long changes that only add header lengths wait before the index cache is rewritten.
    pub metadata_save_delay: Duration,
}

impl LibraryConfig {
    /// The configuration of the application: standard locations, background loading, gentle scanning.
    pub fn new(support_dir: impl Into<PathBuf>) -> LibraryConfig {
        LibraryConfig {
            support_dir: support_dir.into(),
            user_kits_dir: kit::default_user_directory(),
            default_roots: None,
            load_cache_synchronously: false,
            scan_pause: Duration::from_millis(8),
            metadata_pause: Duration::from_millis(4),
            watch_interval: Some(Duration::from_secs(2)),
            metadata_save_delay: Duration::from_secs(45),
        }
    }

    /// A configuration for tests and tools: only `roots`, no system content, no pauses, no
    /// polling, cache loaded before `open` returns.
    pub fn isolated(support_dir: impl Into<PathBuf>, roots: Vec<PathBuf>) -> LibraryConfig {
        LibraryConfig {
            support_dir: support_dir.into(),
            user_kits_dir: None,
            default_roots: Some(roots),
            load_cache_synchronously: true,
            scan_pause: Duration::ZERO,
            metadata_pause: Duration::ZERO,
            watch_interval: None,
            metadata_save_delay: Duration::from_secs(45),
        }
    }
}

struct ScanToken {
    id: String,
    cancelled: AtomicBool,
}

#[derive(Default)]
struct State {
    roots: Vec<Root>,
    excluded: BTreeSet<String>,
    items: HashMap<Box<str>, Arc<Item>>,
    snapshot: Option<Arc<Vec<Arc<Item>>>>,
    favorites: Arc<HashSet<String>>,
    overrides: HashMap<String, Override>,
    metadata_read_error: Option<String>,
    active_scan: Option<Arc<ScanToken>>,
    last_errors: Vec<String>,
    root_reports: BTreeMap<String, RootReport>,
    verified_expansion_roots: HashSet<String>,
    unavailable_roots: BTreeSet<String>,
    checked_collection_roots: HashSet<String>,
    pending_collections: VecDeque<String>,
    queued_collections: HashSet<String>,
    pending_durations: VecDeque<String>,
    queued_durations: HashSet<String>,
    metadata_worker_scheduled: bool,
    index_revision: u64,
    cache_write_requested: bool,
    cache_write_scheduled: bool,
    cache_save_due: Option<Instant>,
}

impl State {
    fn changed(&mut self) {
        self.index_revision = self.index_revision.wrapping_add(1);
    }
    fn items_changed(&mut self) {
        self.snapshot = None;
        self.changed();
    }
    fn reports(&self) -> Vec<RootReport> {
        self.root_reports.values().cloned().collect()
    }
}

struct Inner {
    config: LibraryConfig,
    tables: Tables,
    state: Mutex<State>,
    events: Mutex<Option<EventSink>>,
    search: Mutex<SearchCache>,
    search_generation: AtomicU64,
    running_searches: AtomicUsize,
    peak_searches: AtomicUsize,
    shutdown: AtomicBool,
    scan_worker: Worker,
    disk_worker: Worker,
    metadata_worker: Worker,
    nks: NksCatalog,
    kits: KitCatalog,
}

/// The sound library: library folders, the index and its search, favorites and labels, the
/// NKS preset catalogue, the kit catalogue, and reading and saving kits.
///
/// Every method that a bridge command maps to takes the command's payload and returns its
/// result, so the host registers each in one line. Methods return quickly: scanning, header
/// reading and cache writes run on worker threads the library owns, and report through the
/// [`EventSink`]. `search`, `nks_browse` and `kits_browse` compute in the calling thread
/// (milliseconds to tens of milliseconds on large libraries) and belong on the host's
/// blocking lane.
///
/// Dropping the library cancels a running scan, lets the workers finish their current job
/// and writes pending index changes.
pub struct Library {
    inner: Arc<Inner>,
    watcher: Mutex<Option<std::thread::JoinHandle<()>>>,
}

const SAMPLE_IGNORED_DIRECTORIES: [&str; 9] = ["node_modules", ".git", ".svn", "__macosx", "$recycle.bin", "system volume information", ".trashes", ".spotlight-v100", ".fseventsd"];
const SOFTWARE_PACKAGES: [&str; 9] = ["app", "component", "vst", "vst3", "framework", "bundle", "xcodeproj", "photoslibrary", "musiclibrary"];

impl Library {
    /// Opens the library in `config.support_dir`: reads the roots, favorites and labels, loads
    /// the index cache (in the background unless configured otherwise) and starts the workers.
    pub fn open(config: LibraryConfig) -> Result<Library> {
        fs::create_dir_all(&config.support_dir).map_err(|error| Error::io("create", &config.support_dir, &error))?;
        let discover = config.default_roots.is_none();
        let artwork_root = if discover { crate::nks::artwork::default_root() } else { None }.unwrap_or_else(|| config.support_dir.join("no-artwork"));
        let artwork = Arc::new(NksArtwork::new(&artwork_root));
        let mut state = State::default();

        // Roots: the saved list, else the defaults. Standard locations that appeared since are added.
        let defaults: Vec<Root> = match &config.default_roots {
            Some(roots) => roots.iter().map(|root| Root::new(&root.to_string_lossy(), "configured")).collect(),
            None => crate::roots::discover_roots(config.user_kits_dir.as_deref(), &crate::nks::catalog::system_registrations()),
        };
        let saved = index::load_roots(&config.support_dir);
        let had_roots_file = saved.is_some();
        if let Some((roots, excluded)) = saved {
            state.excluded = excluded.into_iter().collect();
            state.roots = roots.into_iter().filter(|root| !state.excluded.contains(&root.path)).collect();
        }
        for root in defaults {
            if !state.excluded.contains(&root.path) && !state.roots.iter().any(|known| known.path == root.path) {
                state.roots.push(root);
            }
        }

        // The user's labels are authoritative: an unreadable file is preserved and edits are refused.
        let labels_path = config.support_dir.join("metadata.json");
        if labels_path.exists() {
            let loaded = (|| -> Result<HashMap<String, Override>> {
                let data = util::read_bounded(&labels_path, 32 * 1024 * 1024).ok_or_else(|| Error::invalid("Library metadata exceeds the supported size."))?;
                let file: MetadataFile = serde_json::from_slice(&data).map_err(|error| Error::invalid(error.to_string()))?;
                if file.version != 1 {
                    return Err(Error::invalid("This library metadata version is unsupported."));
                }
                let mut validated = HashMap::new();
                for (path, value) in file.items {
                    let mut labels = Override::default();
                    labels.merge(value.to_value().as_object().unwrap_or(&Map::new()))?;
                    validated.insert(normalized(&path), labels);
                }
                Ok(validated)
            })();
            match loaded {
                Ok(overrides) => state.overrides = overrides,
                Err(error) => state.metadata_read_error = Some(format!("Could not read saved library metadata. The file has been preserved: {error}")),
            }
        }
        let favorites: Vec<String> = util::read_bounded(&config.support_dir.join("favorites.json"), 64 * 1024 * 1024).and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default();
        state.favorites = Arc::new(favorites.into_iter().collect());

        let inner = Arc::new(Inner {
            nks: NksCatalog::new(&config.support_dir, discover, &[], artwork.clone()),
            kits: KitCatalog::new(Some(&config.support_dir), artwork),
            tables: Tables::default(),
            state: Mutex::new(state),
            events: Mutex::new(None),
            search: Mutex::new(SearchCache::default()),
            search_generation: AtomicU64::new(0),
            running_searches: AtomicUsize::new(0),
            peak_searches: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            scan_worker: Worker::new("st-library-scan"),
            disk_worker: Worker::new("st-library-disk"),
            metadata_worker: Worker::new("st-library-metadata"),
            config,
        });
        if !had_roots_file {
            inner.save_roots();
        }
        // The catalogues report through the library's sink and learn the library's NKS files from it.
        let weak = Arc::downgrade(&inner);
        let forward: EventSink = {
            let weak = weak.clone();
            Arc::new(move |name: &str, payload: Value| {
                if let Some(inner) = weak.upgrade() {
                    inner.emit(name, payload);
                }
            })
        };
        inner.nks.set_event_sink(forward.clone());
        inner.kits.set_event_sink(forward);
        {
            let weak = weak.clone();
            inner.nks.set_extra_provider(Arc::new(move || match weak.upgrade() {
                Some(inner) => {
                    let state = lock(&inner.state);
                    Extra { presets: state.items.values().filter(|item| item.kind == Kind::Nks).map(|item| item.path.to_string()).collect(), roots: state.roots.iter().map(|root| root.path.clone()).collect() }
                }
                None => Extra::default(),
            }));
        }
        if inner.config.load_cache_synchronously {
            inner.load_cache();
        } else {
            let loading = inner.clone();
            inner.scan_worker.submit(move || loading.load_cache());
        }
        let watcher = inner.config.watch_interval.map(|interval| {
            std::thread::Builder::new()
                .name("st-library-watch".to_string())
                .spawn(move || Inner::watch(weak, interval))
                .ok()
        });
        Ok(Library { inner, watcher: Mutex::new(watcher.flatten()) })
    }

    /// Sets where events go. Events emitted before a sink is set are dropped.
    pub fn set_event_sink(&self, sink: impl Fn(&str, Value) + Send + Sync + 'static) {
        *lock(&self.inner.events) = Some(Arc::new(sink));
    }

    /// The folder kits are saved to.
    pub fn user_kits_dir(&self) -> Option<&Path> {
        self.inner.config.user_kits_dir.as_deref()
    }

    /// Blocks until the work queued so far has finished: scans, header reads, catalogue
    /// refreshes and file writes. For tests and for an orderly shutdown.
    pub fn wait_idle(&self) {
        for _ in 0..3 {
            self.inner.scan_worker.wait_idle();
            self.inner.metadata_worker.wait_idle();
            self.inner.nks.wait_idle();
            self.inner.kits.wait_idle();
            self.inner.disk_worker.wait_idle();
        }
    }

    // ---- library.* --------------------------------------------------------------------------

    /// `library.roots`: the library folders with their scan status.
    pub fn roots(&self, _: Empty) -> Result<Vec<RootInfo>> {
        Ok(self.inner.root_infos())
    }

    /// `library.diagnostics`: a shallow look at folders from this process.
    pub fn diagnostics(&self, request: DiagnosticsRequest) -> Result<Vec<RootProbe>> {
        let paths = request.roots.unwrap_or_else(|| lock(&self.inner.state).roots.iter().map(|root| root.path.clone()).collect());
        if paths.len() > 128 {
            return Err(Error::invalid("Inspect up to 128 library roots at a time."));
        }
        Ok(paths.iter().map(|path| probe_root(&normalized(path))).collect())
    }

    /// `library.status`: whether a scan runs, the item count, and the last scan's outcome.
    pub fn status(&self, _: Empty) -> Result<ScanStatus> {
        let state = lock(&self.inner.state);
        Ok(ScanStatus {
            scanning: state.active_scan.is_some(),
            count: state.items.len(),
            errors: Some(state.last_errors.clone()),
            root_reports: Some(state.reports()),
            diagnostics_path: Some(self.inner.config.support_dir.join("diagnostics.json").to_string_lossy().into_owned()),
            ..Default::default()
        })
    }

    /// `library.scan`: scans the given folders (adding them to the library) or every root.
    /// Returns at once; progress arrives as `library.progress` and `library.results` events.
    /// A new scan replaces a running one.
    pub fn scan(&self, request: ScanRequest) -> Result<ScanStatus> {
        self.inner.begin_scan(request.roots)
    }

    /// `library.cancel`: stops the running scan. What it indexed so far stays.
    pub fn cancel(&self, _: Empty) -> Result<ScanStatus> {
        let result = {
            let mut state = lock(&self.inner.state);
            if let Some(token) = state.active_scan.take() {
                token.cancelled.store(true, Ordering::SeqCst);
            }
            for report in state.root_reports.values_mut().filter(|report| report.status == "scanning" || report.status == "pending") {
                report.status = "cancelled".to_string();
            }
            ScanStatus { scanning: false, count: state.items.len(), cancelled: Some(true), errors: Some(state.last_errors.clone()), root_reports: Some(state.reports()), ..Default::default() }
        };
        self.inner.persist_cache();
        self.inner.persist_diagnostics();
        self.inner.emit_status("library.progress", &result);
        Ok(result)
    }

    /// `library.search`. Searches run one at a time; a search that a newer one overtook
    /// returns at once with `cancelled: true`.
    pub fn search(&self, request: SearchRequest) -> Result<SearchResponse> {
        let inner = &self.inner;
        let generation = inner.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut cache = lock(&inner.search);
        let superseded = || inner.search_generation.load(Ordering::SeqCst) != generation;
        if superseded() {
            return Ok(inner.cancelled_search());
        }
        inner.refresh_availability();
        let snapshot = inner.snapshot();
        let running = inner.running_searches.fetch_add(1, Ordering::SeqCst) + 1;
        inner.peak_searches.fetch_max(running, Ordering::SeqCst);
        let outcome = search::search(&request, &snapshot, &inner.tables, &mut cache, &superseded);
        inner.running_searches.fetch_sub(1, Ordering::SeqCst);
        drop(cache);
        match outcome {
            Some(outcome) => {
                inner.queue_metadata(&outcome.duration_candidates, &[]);
                Ok(outcome.response)
            }
            None => Ok(inner.cancelled_search()),
        }
    }

    /// `library.metadata.edit`: sets or restores the user's labels of up to 500 indexed sounds.
    /// The labels file is written before results change; a failed write changes nothing.
    pub fn edit_metadata(&self, request: MetadataEditRequest) -> Result<MetadataEditResponse> {
        self.inner.edit_metadata(request)
    }

    /// `library.favorite`.
    pub fn favorite(&self, request: FavoriteRequest) -> Result<FavoriteResponse> {
        if request.path.is_empty() {
            return Err(Error::invalid("A file path is required to change a favorite."));
        }
        let path = normalized(&request.path);
        let favorites = {
            let mut state = lock(&self.inner.state);
            let favorites = Arc::make_mut(&mut state.favorites);
            if request.favorite {
                favorites.insert(path.clone());
            } else {
                favorites.remove(&path);
            }
            state.changed();
            state.favorites.clone()
        };
        let inner = self.inner.clone();
        self.inner.disk_worker.submit(move || {
            let mut sorted: Vec<&String> = favorites.iter().collect();
            sorted.sort();
            if let Err(error) = util::write_atomic(&inner.config.support_dir.join("favorites.json"), &serde_json::to_vec(&sorted).unwrap_or_default()) {
                inner.emit("library.error", json!({ "error": format!("Could not save favorites: {error}") }));
            }
        });
        let result = FavoriteResponse { path, favorite: request.favorite };
        self.inner.emit("library.favorite", serde_json::to_value(&result).unwrap_or(Value::Null));
        Ok(result)
    }

    /// `library.state.get`: what the browser asked to remember, or an empty object.
    pub fn state_get(&self, _: Empty) -> Result<Value> {
        let value = util::read_bounded(&self.inner.config.support_dir.join("browser-state.json"), 256 * 1024).and_then(|data| serde_json::from_slice::<Value>(&data).ok());
        Ok(value.filter(Value::is_object).unwrap_or_else(|| json!({})))
    }

    /// `library.state.set`: remembers the browser's state (a JSON object of at most 256 KiB).
    pub fn state_set(&self, request: StateRequest) -> Result<Saved> {
        if !request.state.is_object() {
            return Err(Error::invalid("Browser state must be a JSON object."));
        }
        let data = serde_json::to_vec(&request.state).map_err(|_| Error::invalid("Browser state must be a JSON object."))?;
        if data.len() > 256 * 1024 {
            return Err(Error::invalid("Browser state is too large to remember."));
        }
        let inner = self.inner.clone();
        self.inner.disk_worker.submit(move || {
            if let Err(error) = util::write_atomic(&inner.config.support_dir.join("browser-state.json"), &data) {
                inner.emit("library.error", json!({ "error": format!("Could not remember browser state: {error}") }));
            }
        });
        Ok(Saved { saved: true })
    }

    /// `library.removeRoot`: removes a folder and its items from the library. Standard
    /// locations stay removed until they are scanned explicitly again.
    pub fn remove_root(&self, request: PathRequest) -> Result<Vec<RootInfo>> {
        let path = normalized(&request.path.ok_or_else(|| Error::invalid("A library root path is required."))?);
        {
            let mut state = lock(&self.inner.state);
            if let Some(token) = state.active_scan.take() {
                token.cancelled.store(true, Ordering::SeqCst);
            }
            state.roots.retain(|root| root.path != path);
            state.items.retain(|item, _| !is_inside(item, &path));
            state.excluded.insert(path.clone());
            state.root_reports.remove(&path);
            state.items_changed();
        }
        self.inner.save_roots();
        self.inner.persist_cache();
        Ok(self.inner.root_infos())
    }

    /// `library.nks.browse`: the preset catalogue by product, bank, type and character.
    pub fn nks_browse(&self, request: NksBrowseRequest) -> Result<NksBrowseResponse> {
        let favorites = lock(&self.inner.state).favorites.clone();
        Ok(self.inner.nks.browse(&request, &favorites))
    }

    /// `library.nks.rescan`: refreshes the preset catalogue in the background.
    pub fn nks_rescan(&self, _: Empty) -> Result<NksStatus> {
        self.inner.nks.refresh();
        Ok(self.inner.nks.status())
    }

    /// `library.nks.status`.
    pub fn nks_status(&self, _: Empty) -> Result<NksStatus> {
        Ok(self.inner.nks.status())
    }

    /// `library.nks.artwork`: product pictures for artwork keys of browse results, as data URLs.
    pub fn nks_artwork(&self, request: ArtworkRequest) -> Result<Value> {
        let keys: Vec<String> = request.keys.iter().take(400).map(|key| key.chars().take(512).collect()).collect();
        let variant = request.variant.unwrap_or_else(|| "tile".to_string());
        Ok(json!({ "images": self.inner.nks.artwork().thumbnails(&keys, &variant), "variant": variant }))
    }

    /// `library.nks`: the metadata of one preset file.
    pub fn nks_metadata(&self, request: PathRequest) -> Result<NksMetadata> {
        let path = request.path.ok_or_else(|| Error::invalid("Choose an NKS preset first."))?;
        if !["nksf", "nksfx"].contains(&extension(&path).as_str()) {
            return Err(Error::invalid("This file is not an .nksf or .nksfx preset."));
        }
        NksMetadata::read(&normalized(&path))
    }

    /// The NKS part of `plugin.loadNKS`: the preset's plug-in identity, state bytes and
    /// controller pages. The host finds and loads the plug-in and applies the state.
    pub fn nks_preset(&self, path: &str) -> Result<NksHostPreset> {
        if !["nksf", "nksfx"].contains(&extension(path).as_str()) {
            return Err(Error::invalid("This file is not an .nksf or .nksfx preset."));
        }
        NksHostPreset::read(&normalized(path))
    }

    /// `library.kits.browse`: kits and Maschine groups by expansion, bank and type.
    pub fn kits_browse(&self, request: KitsBrowseRequest) -> Result<KitsBrowseResponse> {
        self.inner.refresh_availability();
        let (sources, favorites, scanning, errors) = {
            let state = lock(&self.inner.state);
            let names: HashMap<String, String> = state.roots.iter().map(|root| (root.path.clone(), root.name.clone())).collect();
            let sources: Vec<Source> = state
                .items
                .values()
                .filter(|item| item.kind == Kind::Kit)
                .map(|item| {
                    let root = self.inner.tables.value(Field::Root, item.root);
                    Source {
                        path: item.path.to_string(),
                        name: item.name.to_string(),
                        pack: self.inner.tables.value(Field::Pack, item.pack).to_string(),
                        root_name: kit::catalog::root_display_name(&root, &names),
                        category: self.inner.tables.value(Field::Category, item.category).to_string(),
                        size: item.size,
                        modified: item.modified,
                        tags: item.tags.iter().map(|tag| self.inner.tables.value(Field::Tag, *tag).to_string()).collect(),
                        preview_path: item.preview.as_deref().map(String::from),
                        sound_count: item.kit_sound_count,
                        metadata_error: item.kit_error.as_deref().map(String::from),
                        available: !state.unavailable_roots.contains(&*root),
                    }
                })
                .collect();
            (sources, state.favorites.clone(), state.active_scan.is_some(), state.last_errors.clone())
        };
        let mut result = self.inner.kits.browse(&request, &sources, &favorites);
        result.scanning = scanning;
        result.errors = errors;
        Ok(result)
    }

    /// `library.kit`: the browsing metadata of one kit package or group file.
    pub fn kit_metadata(&self, request: PathRequest) -> Result<KitMetadata> {
        kit::metadata(&request.path.ok_or_else(|| Error::invalid("Choose a kit first."))?, None)
    }

    // ---- kit.* ------------------------------------------------------------------------------

    /// `kit.read`: opens a `.stkit` kit or imports a Maschine `.mxgrp` group as a group project
    /// the page inserts into the session.
    pub fn kit_read(&self, request: PathRequest) -> Result<KitReadResponse> {
        let path = request.path.filter(|path| st_project::media::is_absolute(path)).ok_or_else(|| Error::invalid("Choose an available kit from the browser."))?;
        read_kit(&path)
    }

    /// `kit.detail`: a kit's metadata, pads and what to expect when loading it.
    pub fn kit_detail(&self, request: KitDetailRequest) -> Result<KitDetail> {
        let path = request.path.filter(|path| st_project::media::is_absolute(path)).ok_or_else(|| Error::invalid("Choose an available kit from the browser."))?;
        let contents = read_kit(&path)?;
        let mut warnings = contents.warnings.clone();
        let mut missing_plugins: Vec<String> = Vec::new();
        if let Some(available) = &request.available_components {
            let available: HashSet<&str> = available.iter().map(String::as_str).collect();
            for plugin in contents.group_project.get("audioPlugins").and_then(Value::as_array).into_iter().flatten() {
                let component = plugin.get("component").and_then(Value::as_str);
                // Built-in effects are part of the application.
                if component.is_some_and(|component| component.starts_with("stfx:") || available.contains(component)) {
                    continue;
                }
                missing_plugins.push(plugin.get("name").and_then(Value::as_str).or(component).unwrap_or("Unknown plug-in").to_string());
            }
            let unique: BTreeSet<&String> = missing_plugins.iter().collect();
            warnings.extend(unique.into_iter().map(|name| format!("Plug-in unavailable on this computer: {name}")));
        }
        let mut metadata = contents.metadata;
        metadata["path"] = json!(path);
        if let Some(preview) = kit::preview_path(&path) {
            metadata["previewPath"] = json!(preview);
        }
        if let Some(object) = metadata.as_object_mut() {
            object.remove("compatibility");
        }
        let summary = if warnings.is_empty() { "Ready to load" } else { "Review import notes" }.to_string();
        Ok(KitDetail { metadata, sounds: contents.sounds, compatibility: KitCompatibility { summary, loadable: true, warnings, missing_plugins } })
    }

    /// `kit.save`: saves a group of the session as a kit package in the user's kit folder and
    /// rescans that folder. Save (a `path`) replaces the kit's own package; Save As (a `name`)
    /// reports an existing kit of that name unless `overwrite` is set.
    ///
    /// The package gets no audio preview; [`Library::kit_save_with_preview`] is the same save
    /// with one.
    pub fn kit_save(&self, request: KitSaveRequest) -> Result<KitSaveResponse> {
        self.kit_save_with_preview(request, None)
    }

    /// `kit.save` with the kit's audio preview: `preview` renders it into the package once it
    /// is certain that the kit will be written (not when the answer is "a kit of that name
    /// exists" or "choose a name"). A preview that cannot be rendered is a warning of the
    /// saved kit, not a failure.
    pub fn kit_save_with_preview(&self, request: KitSaveRequest, preview: Option<kit::PreviewRenderer>) -> Result<KitSaveResponse> {
        let group_id = request.group_id.as_deref().filter(|id| !id.is_empty()).ok_or_else(|| Error::invalid("Select a kit to save."))?;
        let directory = self.inner.config.user_kits_dir.clone().ok_or_else(|| Error::invalid("No kit folder is configured."))?;
        fs::create_dir_all(&directory).map_err(|error| Error::io("create", &directory, &error))?;
        let library_root = fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone());
        let named = request.name.as_deref().map(|name| name.trim().replace(['/', ':', '\\'], "-"));
        if named.as_deref().is_some_and(|name| name.is_empty() || name.chars().count() > 200 || name.starts_with('.')) {
            return Err(Error::invalid("Choose a kit name of up to 200 characters."));
        }
        let mut chosen: Option<PathBuf> = None;
        if let Some(path) = &request.path {
            let target = PathBuf::from(normalized(path));
            // Only packages in the user's kit folder are replaced in place; Save of any other package becomes Save As.
            let own = st_project::media::is_absolute(path)
                && extension(path) == "stkit"
                && target.parent().and_then(|folder| fs::canonicalize(folder).ok()).is_some_and(|folder| folder.starts_with(&library_root));
            if request.overwrite && !own {
                return Ok(KitSaveResponse::NeedsName { needs_name: true, path: path.clone() });
            }
            chosen = Some(target);
        } else if let Some(name) = &named {
            let target = directory.join(format!("{name}.stkit"));
            if target.exists() && !request.overwrite {
                return Ok(KitSaveResponse::Exists { exists: true, path: target.to_string_lossy().into_owned(), name: name.clone() });
            }
            chosen = Some(target);
        }
        let extracted = kit::group::extract_group(&request.project, group_id)?;
        let mut header = extracted.metadata;
        if let Some(name) = &named {
            header.insert("name".to_string(), json!(name));
        }
        let replace = chosen.is_some() && request.overwrite;
        let destination = match chosen {
            Some(destination) => destination,
            None => {
                let safe = header.get("name").and_then(Value::as_str).unwrap_or("Kit").replace(['/', ':', '\\'], "-");
                let mut candidate = directory.join(format!("{safe}.stkit"));
                let mut suffix = 2;
                while candidate.exists() {
                    candidate = directory.join(format!("{safe} {suffix}.stkit"));
                    suffix += 1;
                }
                candidate
            }
        };
        let destination = destination.to_string_lossy().into_owned();
        // A kit saved over itself stays the same kit: its identity and creation date are kept.
        if replace {
            if let Ok(existing) = kit::metadata(&destination, None) {
                if let Some(id) = existing.id {
                    header.insert("id".to_string(), json!(id));
                }
                if let Some(created) = existing.created_at {
                    header.insert("createdAt".to_string(), json!(created));
                }
            }
        }
        let options = kit::SaveOptions { copy_samples: request.copy_samples, relative_to: request.project_path.as_deref().map(PathBuf::from), overwrite: replace, cancel: None, preview };
        let mut saved = kit::save(&extracted.project, &header, &destination, &options)?;
        saved.warnings.splice(0..0, extracted.warnings);
        let _ = self.inner.begin_scan(Some(vec![directory.to_string_lossy().into_owned()]));
        Ok(KitSaveResponse::Saved(saved))
    }

    /// The most searches that ever computed at the same time (always 0 or 1).
    #[doc(hidden)]
    pub fn peak_concurrent_searches(&self) -> usize {
        self.inner.peak_searches.load(Ordering::SeqCst)
    }

    /// Replaces the index with synthetic records (benchmarks and stress tests): `(path, root,
    /// name, category, pack, tags, bpm, key, duration)` per item.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)]
    pub fn load_synthetic(&self, records: impl Iterator<Item = (String, String, String, String, String, Vec<String>, Option<f64>, Option<String>, Option<f64>)>) {
        let mut items: HashMap<Box<str>, Arc<Item>> = HashMap::new();
        for (path, root, name, category, pack, tags, bpm, key, duration) in records {
            let kind = Kind::of_extension(&extension(&path)).unwrap_or(Kind::Sample);
            let record = Record { path, root, name, kind, category, tags, pack, bpm, key, size: 1024, modified: 1.0, metadata_source: "fixture".to_string(), preview_path: None, duration, duration_checked: duration.is_some(), kit_sound_count: None, kit_metadata_error: None };
            items.insert(record.path.as_str().into(), Arc::new(Item::new(&record, &self.inner.tables)));
        }
        let mut state = lock(&self.inner.state);
        state.items = items;
        state.items_changed();
    }

    /// Writes the index cache now and waits for it.
    #[doc(hidden)]
    pub fn flush_cache(&self) {
        self.inner.persist_cache();
        self.inner.disk_worker.wait_idle();
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        let pending = {
            let mut state = lock(&self.inner.state);
            if let Some(token) = state.active_scan.take() {
                token.cancelled.store(true, Ordering::SeqCst);
            }
            state.cache_save_due.take().is_some()
        };
        if let Some(handle) = lock(&self.watcher).take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
        self.inner.scan_worker.stop();
        self.inner.metadata_worker.stop();
        if pending {
            self.inner.persist_cache();
        }
        self.inner.disk_worker.wait_idle();
        self.inner.disk_worker.stop();
    }
}

/// Opens a kit or imports a group, as `kit.read` returns it.
fn read_kit(path: &str) -> Result<KitReadResponse> {
    if extension(path) == "mxgrp" {
        return crate::maschine::read_group(path, None);
    }
    let contents = kit::read(path)?;
    Ok(KitReadResponse {
        path: contents.path,
        metadata: serde_json::to_value(&contents.metadata).unwrap_or(Value::Null),
        group_project: contents.project,
        sounds: contents.sounds,
        warnings: contents.warnings,
        report: None,
    })
}

fn probe_attributes(path: &Path) -> std::io::Result<ProbeAttributes> {
    let metadata = fs::symlink_metadata(path)?;
    let followed = fs::metadata(path).ok();
    let is_directory = followed.as_ref().is_some_and(|metadata| metadata.is_dir());
    Ok(ProbeAttributes {
        is_directory,
        is_regular_file: followed.as_ref().is_some_and(|metadata| metadata.is_file()),
        is_symbolic_link: metadata.file_type().is_symlink(),
        is_hidden: path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')),
        is_readable: if is_directory { fs::read_dir(path).is_ok() } else { fs::File::open(path).is_ok() },
    })
}

/// A shallow look at `path` from this process.
pub(crate) fn probe_root(path: &str) -> RootProbe {
    let folder = Path::new(path);
    let mut result = RootProbe { path: path.to_string(), available: is_directory(path), can_read_directory: false, entry_count: 0, sample_entries: Vec::new(), attributes: None, metadata_error: None, error: None };
    match probe_attributes(folder) {
        Ok(attributes) => result.attributes = Some(attributes),
        Err(error) => result.metadata_error = Some(io_reason(&error)),
    }
    match fs::read_dir(folder) {
        Ok(entries) => {
            let mut names: Vec<String> = entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
            result.can_read_directory = true;
            result.entry_count = names.len();
            names.retain(|name| !name.starts_with('.'));
            names.sort();
            result.sample_entries = names
                .into_iter()
                .take(8)
                .map(|name| match probe_attributes(&folder.join(&name)) {
                    Ok(attributes) => ProbeEntry { name, attributes, error: None },
                    Err(error) => ProbeEntry { name, attributes: ProbeAttributes::default(), error: Some(io_reason(&error)) },
                })
                .collect();
        }
        Err(error) => result.error = Some(io_reason(&error)),
    }
    result
}

/// Everything a scan learns about one file.
fn make_record(path: &str, root: &Root, kind: Kind, size: u64, modified: f64) -> Record {
    let inferred = infer_path(path, &root.path, &root.name);
    let mut record = Record {
        path: path.to_string(),
        root: root.path.clone(),
        name: inferred.name,
        kind,
        category: if kind == Kind::Kit { "Kit".to_string() } else { inferred.metadata.category },
        tags: inferred.metadata.tags,
        pack: inferred.pack,
        bpm: inferred.metadata.bpm,
        // Preset and kit names are not musical keys ("A Fading Dream").
        key: if kind == Kind::Sample { inferred.metadata.key } else { None },
        size,
        modified,
        metadata_source: "filename and folders".to_string(),
        preview_path: None,
        duration: None,
        duration_checked: false,
        kit_sound_count: None,
        kit_metadata_error: None,
    };
    let merged = |own: &[String], more: &[String]| -> Vec<String> { own.iter().chain(more).cloned().collect::<BTreeSet<_>>().into_iter().collect() };
    match kind {
        Kind::Nks => {
            if let Ok(parsed) = NksMetadata::read(path) {
                if !parsed.name.is_empty() {
                    record.name = parsed.name.clone();
                }
                // The full NKS type path stays searchable as a tag; the category joins the sample categories.
                record.tags = merged(&record.tags, &[parsed.categories.clone(), parsed.tags.clone()].concat());
                if let Some(first) = parsed.categories.first().filter(|first| !first.is_empty()) {
                    record.category = nks_category(first);
                    record.tags.retain(|tag| tag != "Other");
                }
                record.preview_path = parsed.preview_path;
                record.metadata_source = "NKS metadata and folders".to_string();
            }
        }
        Kind::Kit => match kit::metadata(path, Some(&root.path)) {
            Ok(parsed) => {
                if !parsed.name.is_empty() {
                    record.name = parsed.name;
                }
                if !parsed.kit_type.is_empty() {
                    record.category = parsed.kit_type;
                }
                // A SuperTracker kit's tags are its own; words guessed from its folders are not.
                record.tags = if extension(path) == "stkit" { parsed.tags } else { merged(&record.tags, &parsed.tags) };
                if let Some(expansion) = parsed.expansion.filter(|expansion| !expansion.is_empty()) {
                    record.pack = clean_pack(&expansion);
                }
                record.preview_path = parsed.preview_path;
                record.kit_sound_count = parsed.sound_count.map(|count| count as u32);
                record.metadata_source = parsed.metadata_source;
            }
            Err(error) => record.kit_metadata_error = Some(error.to_string()),
        },
        Kind::Sample => {}
    }
    record
}

impl Inner {
    fn emit(&self, name: &str, payload: Value) {
        if self.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let sink = lock(&self.events).clone();
        if let Some(sink) = sink {
            sink(name, payload);
        }
    }

    fn emit_status(&self, name: &str, status: &ScanStatus) {
        self.emit(name, serde_json::to_value(status).unwrap_or(Value::Null));
    }

    fn cancelled_search(&self) -> SearchResponse {
        SearchResponse { scanning: lock(&self.state).active_scan.is_some(), cancelled: Some(true), ..Default::default() }
    }

    fn save_roots(&self) {
        let (roots, excluded) = {
            let state = lock(&self.state);
            (state.roots.clone(), state.excluded.iter().cloned().collect::<Vec<_>>())
        };
        if let Err(error) = index::save_roots(&self.config.support_dir, &roots, &excluded) {
            self.emit("library.error", json!({ "error": format!("Could not save the library folders: {error}") }));
        }
    }

    fn root_infos(&self) -> Vec<RootInfo> {
        let state = lock(&self.state);
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for item in state.items.values() {
            *counts.entry(item.root).or_insert(0) += 1;
        }
        state
            .roots
            .iter()
            .map(|root| {
                let report = state.root_reports.get(&root.path);
                RootInfo {
                    path: root.path.clone(),
                    name: root.name.clone(),
                    source: root.source.clone(),
                    available: is_directory(&root.path),
                    scan_status: report.map(|report| report.status.clone()).unwrap_or_else(|| "unscanned".to_string()),
                    scan_errors: report.map(|report| report.errors.clone()).unwrap_or_default(),
                    indexed_count: counts.get(&self.tables.id(Field::Root, &root.path)).copied().unwrap_or(0),
                }
            })
            .collect()
    }

    /// Checks which roots exist. True when one became available or unavailable. One stat per root.
    fn refresh_availability(&self) -> bool {
        let paths: Vec<String> = lock(&self.state).roots.iter().map(|root| root.path.clone()).collect();
        let offline: BTreeSet<String> = paths.into_iter().filter(|path| !is_directory(path)).collect();
        let mut state = lock(&self.state);
        if offline == state.unavailable_roots {
            return false;
        }
        state.unavailable_roots = offline;
        state.changed();
        true
    }

    /// The availability watcher: mounting or ejecting a drive marks its sounds at once.
    fn watch(inner: Weak<Inner>, interval: Duration) {
        loop {
            std::thread::park_timeout(interval);
            let Some(inner) = inner.upgrade() else { return };
            if inner.shutdown.load(Ordering::SeqCst) {
                return;
            }
            if inner.refresh_availability() {
                let status = {
                    let state = lock(&inner.state);
                    ScanStatus { scanning: state.active_scan.is_some(), count: state.items.len(), volumes: Some(true), errors: Some(state.last_errors.clone()), ..Default::default() }
                };
                inner.emit_status("library.results", &status);
            }
            // Header lengths gathered while browsing are written once browsing has settled.
            let due = lock(&inner.state).cache_save_due.is_some_and(|due| Instant::now() >= due);
            if due {
                lock(&inner.state).cache_save_due = None;
                inner.persist_cache();
            }
        }
    }

    fn snapshot(&self) -> Snapshot {
        let mut state = lock(&self.state);
        let items = match &state.snapshot {
            Some(items) => items.clone(),
            None => {
                let items: Arc<Vec<Arc<Item>>> = Arc::new(state.items.values().cloned().collect());
                state.snapshot = Some(items.clone());
                items
            }
        };
        Snapshot {
            items,
            favorites: state.favorites.clone(),
            scanning: state.active_scan.is_some(),
            errors: state.last_errors.clone(),
            revision: state.index_revision,
            reports: state.reports(),
            verified_expansions: state.verified_expansion_roots.clone(),
            queued_durations: state.queued_durations.clone(),
            roots: state.roots.clone(),
            offline: state.unavailable_roots.clone(),
        }
    }

    // ---- cache ------------------------------------------------------------------------------

    fn load_cache(self: &Arc<Self>) {
        let path = self.config.support_dir.join("index.bin");
        let exists = path.exists();
        let restored = util::read_bounded(&path, 1024 * 1024 * 1024).and_then(|bytes| index::decode_cache(&bytes));
        let rebuild = exists && restored.is_none();
        let mut collection_roots: Vec<String> = Vec::new();
        if let Some(cache) = restored {
            let mut state = lock(&self.state);
            let known: HashSet<String> = state.roots.iter().map(|root| root.path.clone()).collect();
            for record in cache.items.iter().filter(|record| known.contains(&record.root)) {
                if state.items.contains_key(record.path.as_str()) {
                    continue;
                }
                let item = Arc::new(Item::new(record, &self.tables));
                if let Some(root) = item.collection_root {
                    collection_roots.push(self.tables.value(Field::CollectionRoot, root).to_string());
                }
                let item = item.applying(state.overrides.get(&record.path), &self.tables);
                state.items.insert(record.path.as_str().into(), item);
            }
            state.verified_expansion_roots.extend(cache.verified_expansion_roots);
            if state.active_scan.is_none() {
                state.last_errors = cache.errors;
                for mut report in cache.reports {
                    // A scan that was running when the application quit did not finish.
                    if report.status == "pending" || report.status == "scanning" {
                        report.status = "cancelled".to_string();
                    }
                    state.root_reports.insert(report.path.clone(), report);
                }
            }
            state.items_changed();
        } else {
            lock(&self.state).changed();
        }
        self.refresh_availability();
        let status = {
            let state = lock(&self.state);
            ScanStatus { scanning: state.active_scan.is_some(), count: state.items.len(), cached: Some(true), rebuilding: rebuild.then_some(true), errors: Some(state.last_errors.clone()), root_reports: Some(state.reports()), ..Default::default() }
        };
        self.emit_status("library.results", &status);
        collection_roots.sort();
        collection_roots.dedup();
        self.queue_metadata(&[], &collection_roots);
        if rebuild {
            // A cache of another version, or a damaged one: the index is rebuilt from the folders.
            let has_roots = !lock(&self.state).roots.is_empty();
            if has_roots {
                let _ = self.begin_scan(None);
            }
        }
    }

    /// Writes the index cache on the disk worker. Requests during a write are folded into one more write.
    fn persist_cache(self: &Arc<Self>) {
        {
            let mut state = lock(&self.state);
            state.cache_write_requested = true;
            if state.cache_write_scheduled {
                return;
            }
            state.cache_write_scheduled = true;
        }
        let inner = self.clone();
        let submitted = self.disk_worker.submit(move || loop {
            let (items, errors, reports, verified) = {
                let mut state = lock(&inner.state);
                if !state.cache_write_requested {
                    state.cache_write_scheduled = false;
                    return;
                }
                state.cache_write_requested = false;
                let items: Vec<Arc<Item>> = state.items.values().map(|item| item.base()).collect();
                let mut verified: Vec<String> = state.verified_expansion_roots.iter().cloned().collect();
                verified.sort();
                (items, state.last_errors.clone(), state.reports(), verified)
            };
            let cache = Cache { items: items.iter().map(|item| item.record(&inner.tables)).collect(), errors, reports, verified_expansion_roots: verified };
            if let Err(error) = util::write_atomic(&inner.config.support_dir.join("index.bin"), &index::encode_cache(&cache)) {
                inner.emit("library.error", json!({ "error": format!("Could not save the library index: {error}") }));
            }
        });
        if !submitted {
            lock(&self.state).cache_write_scheduled = false;
        }
    }

    /// The latest scan's outcome with a probe of every root, separate from the index so access
    /// problems can be inspected without loading it.
    fn persist_diagnostics(self: &Arc<Self>) {
        let (roots, reports, errors) = {
            let state = lock(&self.state);
            (state.roots.iter().map(|root| root.path.clone()).collect::<Vec<_>>(), state.reports(), state.last_errors.clone())
        };
        let inner = self.clone();
        self.disk_worker.submit(move || {
            let probes: Vec<RootProbe> = roots.iter().take(128).map(|path| probe_root(path)).collect();
            let result = json!({ "version": 1, "createdAt": util::iso8601_now(), "rootReports": reports, "errors": errors, "rootProbes": probes });
            if let Err(error) = util::write_atomic(&inner.config.support_dir.join("diagnostics.json"), &serde_json::to_vec_pretty(&result).unwrap_or_default()) {
                inner.emit("library.error", json!({ "error": format!("Could not save library diagnostics: {error}") }));
            }
        });
    }

    // ---- scanning ---------------------------------------------------------------------------

    fn begin_scan(self: &Arc<Self>, paths: Option<Vec<String>>) -> Result<ScanStatus> {
        let requested: Vec<Root> = match paths {
            Some(paths) => {
                if paths.len() > 128 {
                    return Err(Error::invalid("Scan up to 128 library folders at a time."));
                }
                let requested: Vec<Root> = paths.iter().filter(|path| !path.is_empty()).map(|path| Root::new(path, "user")).collect();
                if requested.is_empty() {
                    return Err(Error::invalid("Choose at least one library folder to scan."));
                }
                requested
            }
            None => lock(&self.state).roots.clone(),
        };
        let token = Arc::new(ScanToken { id: util::unique_id(), cancelled: AtomicBool::new(false) });
        let (result, roots_changed, accepted) = {
            let mut state = lock(&self.state);
            if let Some(previous) = state.active_scan.replace(token.clone()) {
                previous.cancelled.store(true, Ordering::SeqCst);
            }
            state.last_errors.clear();
            let mut roots_changed = false;
            let mut accepted = Vec::with_capacity(requested.len());
            for root in &requested {
                state.root_reports.insert(root.path.clone(), RootReport { path: root.path.clone(), status: "pending".to_string(), ..Default::default() });
                // A folder that is already a root keeps its name and source.
                match state.roots.iter().find(|known| known.path == root.path) {
                    Some(known) => accepted.push(known.clone()),
                    None => {
                        state.roots.push(root.clone());
                        accepted.push(root.clone());
                        roots_changed = true;
                    }
                }
                roots_changed |= state.excluded.remove(&root.path);
            }
            state.checked_collection_roots.retain(|path| !requested.iter().any(|root| is_inside(path, &root.path)));
            let roots = accepted.iter().map(|root| ScanRoot { path: root.path.clone(), name: root.name.clone(), source: root.source.clone(), available: is_directory(&root.path) }).collect();
            (ScanStatus { scanning: true, count: state.items.len(), roots: Some(roots), scan_id: Some(token.id.clone()), ..Default::default() }, roots_changed, accepted)
        };
        if roots_changed {
            self.save_roots();
        }
        self.emit_status("library.progress", &result);
        let inner = self.clone();
        self.scan_worker.submit(move || inner.run_scan(accepted, token));
        Ok(result)
    }

    fn is_active(&self, state: &State, token: &Arc<ScanToken>) -> bool {
        state.active_scan.as_ref().is_some_and(|active| Arc::ptr_eq(active, token))
    }

    fn progress(&self, token: &Arc<ScanToken>, root: &str, scanned: u64, skipped: u64, errors: &[String]) {
        let status = {
            let mut state = lock(&self.state);
            if !self.is_active(&state, token) {
                return;
            }
            state.last_errors = errors.to_vec();
            ScanStatus { scanning: true, count: state.items.len(), root: Some(root.to_string()), scanned: Some(scanned), skipped: Some(skipped), errors: Some(errors.to_vec()), root_reports: Some(state.reports()), scan_id: Some(token.id.clone()), ..Default::default() }
        };
        self.emit_status("library.progress", &status);
        self.emit_status("library.results", &status);
    }

    fn flush(self: &Arc<Self>, batch: &mut Vec<Arc<Item>>, token: &Arc<ScanToken>) {
        if batch.is_empty() {
            return;
        }
        let mut collections: Vec<String> = Vec::new();
        {
            let mut state = lock(&self.state);
            if !self.is_active(&state, token) || token.cancelled.load(Ordering::SeqCst) {
                batch.clear();
                return;
            }
            let mut changed = false;
            for item in batch.drain(..) {
                if let Some(root) = item.collection_root {
                    collections.push(self.tables.value(Field::CollectionRoot, root).to_string());
                }
                if state.items.get(&*item.path).is_some_and(|previous| Arc::ptr_eq(previous, &item)) {
                    continue;
                }
                let applied = item.applying(state.overrides.get(&*item.path), &self.tables);
                state.items.insert(item.path.clone(), applied);
                changed = true;
            }
            if changed {
                state.items_changed();
            }
        }
        collections.sort();
        collections.dedup();
        self.queue_metadata(&[], &collections);
    }

    fn run_scan(self: &Arc<Self>, requested: Vec<Root>, token: Arc<ScanToken>) {
        let cancelled = || token.cancelled.load(Ordering::SeqCst);
        let (mut scanned, mut skipped, mut visited) = (0u64, 0u64, 0u64);
        let mut errors: Vec<String> = Vec::new();
        let (mut last_update, mut last_flush) = (Instant::now() - Duration::from_secs(2), Instant::now());
        // Nested roots are walked once, by their parent, so pack inference stays stable.
        let mut sorted = requested.clone();
        sorted.sort_by_key(|root| root.path.len());
        let mut scan_roots: Vec<Root> = Vec::new();
        for root in sorted {
            if !scan_roots.iter().any(|known| is_inside(&root.path, &known.path)) {
                scan_roots.push(root);
            }
        }
        for root in &scan_roots {
            if cancelled() {
                break;
            }
            let mut report = RootReport { path: root.path.clone(), status: "scanning".to_string(), ..Default::default() };
            {
                let mut state = lock(&self.state);
                if self.is_active(&state, &token) {
                    state.root_reports.insert(root.path.clone(), report.clone());
                }
            }
            self.progress(&token, &root.path, scanned, skipped, &errors);
            let failure = |report: &mut RootReport, errors: &mut Vec<String>, message: String| {
                if report.errors.len() < 6 {
                    report.errors.push(message.clone());
                }
                if errors.len() < 30 {
                    errors.push(message);
                }
            };
            let publish = |report: &mut RootReport, status: &str, errors: &[String], scanned: u64, skipped: u64| {
                report.status = status.to_string();
                {
                    let mut state = lock(&self.state);
                    if self.is_active(&state, &token) {
                        state.root_reports.insert(root.path.clone(), report.clone());
                        // Requested folders inside this one were covered by this walk: they take its outcome.
                        for nested in requested.iter().filter(|nested| nested.path != root.path && is_inside(&nested.path, &root.path)) {
                            state.root_reports.insert(nested.path.clone(), RootReport { path: nested.path.clone(), status: status.to_string(), errors: report.errors.clone(), ..Default::default() });
                        }
                        state.last_errors = errors.to_vec();
                    }
                }
                self.progress(&token, &root.path, scanned, skipped, errors);
            };
            if !is_directory(&root.path) {
                failure(&mut report, &mut errors, format!("Library is offline or unavailable: {}", root.path));
                publish(&mut report, "error", &errors, scanned, skipped);
                continue; // Cached entries of a disconnected drive are kept.
            }
            let top_level_count = match fs::read_dir(&root.path) {
                Ok(entries) => entries.count(),
                Err(error) => {
                    failure(&mut report, &mut errors, format!("Cannot read {}: {}. Choose this folder again to grant access.", root.path, io_reason(&error)));
                    publish(&mut report, "error", &errors, scanned, skipped);
                    continue;
                }
            };
            let mut seen: HashSet<Box<str>> = HashSet::new();
            let mut batch: Vec<Arc<Item>> = Vec::new();
            let mut stack: Vec<PathBuf> = vec![PathBuf::from(&root.path)];
            'walk: while let Some(folder) = stack.pop() {
                let entries = match fs::read_dir(&folder) {
                    Ok(entries) => entries,
                    Err(error) => {
                        failure(&mut report, &mut errors, format!("Cannot enumerate {}: {}", folder.display(), io_reason(&error)));
                        continue;
                    }
                };
                for entry in entries {
                    if cancelled() {
                        break 'walk;
                    }
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            failure(&mut report, &mut errors, format!("Cannot enumerate {}: {}", folder.display(), io_reason(&error)));
                            continue;
                        }
                    };
                    visited += 1;
                    report.visited += 1;
                    // A bulk scan in the background leaves CPU and disk to playback.
                    if visited % 256 == 0 && !self.config.scan_pause.is_zero() {
                        std::thread::sleep(self.config.scan_pause);
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') || SAMPLE_IGNORED_DIRECTORIES.contains(&name.to_lowercase().as_str()) {
                        skipped += 1;
                        report.skipped += 1;
                        continue;
                    }
                    let kind = match entry.file_type() {
                        Ok(kind) => kind,
                        Err(error) => {
                            failure(&mut report, &mut errors, format!("Cannot inspect {}: {}", entry.path().display(), io_reason(&error)));
                            skipped += 1;
                            report.skipped += 1;
                            continue;
                        }
                    };
                    if kind.is_symlink() {
                        skipped += 1;
                        report.skipped += 1;
                        continue;
                    }
                    let path = entry.path().to_string_lossy().into_owned();
                    let ext = extension(&path);
                    let kit_package = kind.is_dir() && ext == "stkit";
                    if kind.is_dir() && !kit_package {
                        // Sample expansions can be packages; only software bundles are not descended.
                        if SOFTWARE_PACKAGES.contains(&ext.as_str()) {
                            skipped += 1;
                            report.skipped += 1;
                        } else {
                            stack.push(entry.path());
                        }
                        continue;
                    }
                    if !kit_package && !kind.is_file() {
                        skipped += 1;
                        report.skipped += 1;
                        continue;
                    }
                    let Some(item_kind) = Kind::of_extension(&ext) else { continue };
                    if kind.is_file() && ext == "stkit" {
                        continue;
                    }
                    // Only a kit package's small header affects the index: collected media is never
                    // indexed and project state is never read while scanning.
                    let metadata = if kit_package { fs::metadata(entry.path().join("kit.json")).or_else(|_| entry.metadata()) } else { entry.metadata() };
                    let Ok(metadata) = metadata else {
                        failure(&mut report, &mut errors, format!("Cannot inspect {path}"));
                        skipped += 1;
                        report.skipped += 1;
                        continue;
                    };
                    let size = if kit_package { metadata.len().max(1) } else { metadata.len() };
                    let modified = modified_seconds(&metadata);
                    if size == 0 {
                        skipped += 1;
                        report.skipped += 1;
                        continue;
                    }
                    seen.insert(path.as_str().into());
                    let previous = lock(&self.state).items.get(path.as_str()).cloned();
                    match previous {
                        Some(previous) if previous.size == size && previous.modified == modified => batch.push(previous),
                        _ => batch.push(Arc::new(Item::new(&make_record(&path, root, item_kind, size, modified), &self.tables))),
                    }
                    scanned += 1;
                    report.matched += 1;
                    if batch.len() >= 512 || last_flush.elapsed() > Duration::from_millis(250) {
                        self.flush(&mut batch, &token);
                        last_flush = Instant::now();
                    }
                    if last_update.elapsed() >= Duration::from_secs(1) {
                        {
                            let mut state = lock(&self.state);
                            if self.is_active(&state, &token) {
                                state.root_reports.insert(root.path.clone(), report.clone());
                            }
                        }
                        self.progress(&token, &root.path, scanned, skipped, &errors);
                        last_update = Instant::now();
                    }
                }
            }
            self.flush(&mut batch, &token);
            if cancelled() {
                publish(&mut report, "cancelled", &errors, scanned, skipped);
                continue;
            }
            {
                let mut state = lock(&self.state);
                // Stale entries go only when this root was read in full: a folder that could not
                // be read keeps its cached entries until a clean rescan.
                if self.is_active(&state, &token) && report.errors.is_empty() {
                    let before = state.items.len();
                    state.items.retain(|path, _| !is_inside(path, &root.path) || seen.contains(path));
                    if state.items.len() != before {
                        state.items_changed();
                    }
                }
            }
            if report.matched == 0 && report.errors.is_empty() {
                report.errors.push(format!("No supported audio, NKS or kit files were found in {}. Top-level entries: {top_level_count}; entries inspected: {}.", root.path, report.visited));
                publish(&mut report, "empty", &errors, scanned, skipped);
            } else {
                let status = if report.errors.is_empty() { "complete" } else { "error" };
                publish(&mut report, status, &errors, scanned, skipped);
            }
        }
        let result = {
            let mut state = lock(&self.state);
            if !self.is_active(&state, &token) {
                return;
            }
            state.active_scan = None;
            state.last_errors = errors.clone();
            ScanStatus { scanning: false, count: state.items.len(), scanned: Some(scanned), skipped: Some(skipped), errors: Some(errors), root_reports: Some(state.reports()), cancelled: Some(cancelled()), scan_id: Some(token.id.clone()), root: Some(String::new()), ..Default::default() }
        };
        self.persist_cache();
        self.persist_diagnostics();
        self.emit_status("library.progress", &result);
        self.emit_status("library.results", &result);
        // Library folders can contain NKS presets; the catalogue follows once it is in use.
        if self.nks.is_active() {
            self.nks.refresh();
        }
    }

    // ---- header lengths and expansion checks ------------------------------------------------

    /// Queues header reads (at most 32 per call, 128 in all) and expansion folder checks.
    /// Lengths are read only for visible results or a small batch a duration filter needs,
    /// never for the whole index.
    fn queue_metadata(self: &Arc<Self>, durations: &[String], collections: &[String]) {
        {
            let mut state = lock(&self.state);
            for path in durations.iter().take(32) {
                if state.queued_durations.len() >= 128 {
                    break;
                }
                let wanted = state.items.get(path.as_str()).is_some_and(|item| item.kind == Kind::Sample && !item.duration_checked);
                if wanted && state.queued_durations.insert(path.clone()) {
                    state.pending_durations.push_back(path.clone());
                }
            }
            for path in collections {
                if state.queued_collections.len() >= 4096 {
                    break;
                }
                if !state.checked_collection_roots.contains(path) && state.queued_collections.insert(path.clone()) {
                    state.pending_collections.push_back(path.clone());
                }
            }
            if state.metadata_worker_scheduled || (state.pending_durations.is_empty() && state.pending_collections.is_empty()) {
                return;
            }
            state.metadata_worker_scheduled = true;
        }
        let inner = self.clone();
        if !self.metadata_worker.submit(move || inner.process_metadata()) {
            lock(&self.state).metadata_worker_scheduled = false;
        }
    }

    /// Records a header length. A user edit can have replaced the item while its header was
    /// read: the length lands on the latest labels, and only if the file is still the one read.
    fn complete_duration(&self, path: &str, source: &Arc<Item>, duration: Option<f64>) {
        let mut state = lock(&self.state);
        state.queued_durations.remove(path);
        let Some(latest) = state.items.get(path).cloned() else { return };
        if latest.size == source.size && latest.modified == source.modified && latest.kind == source.kind {
            let updated = latest.replacing_duration(duration, &self.tables);
            state.items.insert(path.into(), updated);
            state.snapshot = None;
        } else if latest.kind == Kind::Sample && !latest.duration_checked {
            state.queued_durations.insert(path.to_string());
            state.pending_durations.push_back(path.to_string());
        }
        // Queue changes invalidate cached pending status even without a new length.
        state.changed();
    }

    fn process_metadata(self: &Arc<Self>) {
        let mut changed = 0usize;
        let mut last_event = Instant::now();
        let emit = |inner: &Inner, changed: usize| {
            let payload = {
                let mut state = lock(&inner.state);
                state.cache_save_due.get_or_insert(Instant::now() + inner.config.metadata_save_delay);
                json!({ "changed": changed, "pending": state.queued_durations.len(), "collectionsPending": state.queued_collections.len() })
            };
            inner.emit("library.metadata", payload);
        };
        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                lock(&self.state).metadata_worker_scheduled = false;
                return;
            }
            let job: Option<(String, Option<Arc<Item>>)> = {
                let mut state = lock(&self.state);
                let mut job = None;
                while let Some(path) = state.pending_durations.pop_front() {
                    match state.items.get(path.as_str()).cloned() {
                        Some(item) => {
                            job = Some((path, Some(item)));
                            break;
                        }
                        None => {
                            state.queued_durations.remove(&path);
                        }
                    }
                }
                if job.is_none() {
                    job = state.pending_collections.pop_front().map(|path| (path, None));
                }
                if job.is_none() {
                    state.metadata_worker_scheduled = false;
                }
                job
            };
            let Some((path, item)) = job else { break };
            match item {
                Some(item) => {
                    if !Path::new(&path).exists() {
                        // A missing file (ejected drive) is not an unreadable header: it is tried again later.
                        let mut state = lock(&self.state);
                        state.queued_durations.remove(&path);
                        state.changed();
                    } else {
                        let duration = header_duration(&item);
                        self.complete_duration(&path, &item, duration);
                    }
                    changed += 1;
                }
                None => {
                    // A folder holding Groups, Sounds and Samples is a Maschine expansion.
                    let available = is_directory(&path);
                    let expansion = available && ["Groups", "Sounds", "Samples"].iter().all(|folder| Path::new(&path).join(folder).is_dir());
                    let mut state = lock(&self.state);
                    state.queued_collections.remove(&path);
                    state.checked_collection_roots.insert(path.clone());
                    let altered = if expansion { state.verified_expansion_roots.insert(path) } else { available && state.verified_expansion_roots.remove(&path) };
                    if altered {
                        state.changed();
                        changed += 1;
                    }
                }
            }
            if changed > 0 && last_event.elapsed() >= Duration::from_millis(500) {
                emit(self, changed);
                changed = 0;
                last_event = Instant::now();
            }
            if !self.config.metadata_pause.is_zero() {
                std::thread::sleep(self.config.metadata_pause);
            }
        }
        if changed > 0 {
            emit(self, changed);
        }
    }

    // ---- labels -----------------------------------------------------------------------------

    fn edit_metadata(self: &Arc<Self>, request: MetadataEditRequest) -> Result<MetadataEditResponse> {
        if let Some(error) = lock(&self.state).metadata_read_error.clone() {
            return Err(Error::invalid(error));
        }
        let invalid = || Error::invalid("Select between 1 and 500 indexed sounds and their metadata changes.");
        let changes = request.changes.ok_or_else(invalid)?;
        if request.paths.is_empty() || request.paths.len() > 500 || request.paths.iter().any(String::is_empty) {
            return Err(invalid());
        }
        let paths: Vec<String> = request.paths.iter().map(|path| normalized(path)).collect::<BTreeSet<_>>().into_iter().collect();
        let (mut next, current_tags) = {
            let state = lock(&self.state);
            if !paths.iter().all(|path| state.items.contains_key(path.as_str())) {
                return Err(Error::invalid("A selected sound is no longer indexed. Refresh the library and try again."));
            }
            let tags: HashMap<String, Vec<String>> =
                paths.iter().map(|path| (path.clone(), state.items[path.as_str()].tags.iter().map(|tag| self.tables.value(Field::Tag, *tag).to_string()).collect())).collect();
            (state.overrides.clone(), tags)
        };
        let mut edits = changes;
        let added_tags = edits.remove("addTags");
        if added_tags.is_some() && edits.contains_key("tags") {
            return Err(Error::invalid("Choose either Add tags or Replace tags."));
        }
        let mut validated_tags = Override::default();
        if let Some(added) = added_tags {
            let mut only_tags = Map::new();
            only_tags.insert("tags".to_string(), added);
            validated_tags.merge(&only_tags)?;
            if validated_tags.tags.is_none() {
                return Err(Error::invalid("Provide the tags to add."));
            }
        }
        for path in &paths {
            let mut value = next.get(path).cloned().unwrap_or_default();
            value.merge(&edits)?;
            if let Some(tags) = &validated_tags.tags {
                // Merged case-insensitively before the resulting tag limit applies.
                let mut seen = HashSet::new();
                let combined: Vec<Value> = current_tags[path].iter().chain(tags).filter(|tag| seen.insert(util::fold(tag))).map(|tag| json!(tag)).collect();
                let mut only_tags = Map::new();
                only_tags.insert("tags".to_string(), Value::Array(combined));
                value.merge(&only_tags)?;
            }
            if value.is_empty() {
                next.remove(path);
            } else {
                next.insert(path.clone(), value);
            }
        }
        let file = MetadataFile { version: 1, items: next.iter().map(|(path, value)| (path.clone(), value.clone())).collect() };
        let data = serde_json::to_vec(&file).map_err(|error| Error::invalid(error.to_string()))?;
        if data.len() > 32 * 1024 * 1024 {
            return Err(Error::invalid("Library metadata exceeds the supported size."));
        }
        util::write_atomic(&self.config.support_dir.join("metadata.json"), &data)?;
        let items = {
            let mut state = lock(&self.state);
            for path in &paths {
                if let Some(item) = state.items.get(path.as_str()).cloned() {
                    let updated = item.applying(next.get(path), &self.tables);
                    state.items.insert(path.as_str().into(), updated);
                }
            }
            state.overrides = next;
            state.items_changed();
            let verified: HashSet<u32> = state.verified_expansion_roots.iter().map(|root| self.tables.id(Field::CollectionRoot, root)).collect();
            paths.iter().filter_map(|path| state.items.get(path.as_str())).map(|item| search::item_row(item, &self.tables, &state.favorites, &verified, &state.queued_durations)).collect()
        };
        self.emit("library.metadata", json!({ "paths": paths, "edited": true }));
        Ok(MetadataEditResponse { items, updated: paths.len() })
    }
}

/// The header length of `item`'s file, if the file is still the one that was indexed.
fn header_duration(item: &Item) -> Option<f64> {
    let path = Path::new(&*item.path);
    let unchanged = || fs::metadata(path).is_ok_and(|metadata| metadata.len() == item.size && modified_seconds(&metadata) == item.modified);
    if !unchanged() {
        return None;
    }
    let duration = crate::probe::header_duration(path)?;
    unchanged().then_some(duration)
}

#[cfg(test)]
impl Library {
    /// Records a header length as the worker would (tests of interleaved edits).
    pub(crate) fn complete_duration_for_test(&self, path: &str, size: u64, modified: f64, duration: Option<f64>) {
        let source = {
            let state = lock(&self.inner.state);
            let mut record = state.items[path].base().record(&self.inner.tables);
            record.size = size;
            record.modified = modified;
            Arc::new(Item::new(&record, &self.inner.tables))
        };
        self.inner.complete_duration(path, &source, duration);
    }

    pub(crate) fn hold_metadata_worker(&self) -> std::sync::mpsc::Sender<()> {
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        self.inner.metadata_worker.submit(move || {
            let _ = receiver.recv();
        });
        sender
    }

    pub(crate) fn set_item_for_test(&self, record: Record) {
        let mut state = lock(&self.inner.state);
        let item = Arc::new(Item::new(&record, &self.inner.tables)).applying(state.overrides.get(&record.path), &self.inner.tables);
        state.items.insert(record.path.as_str().into(), item);
        state.items_changed();
    }

    pub(crate) fn queued_durations_for_test(&self) -> HashSet<String> {
        lock(&self.inner.state).queued_durations.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &str, root: &str, size: u64, modified: f64) -> Record {
        Record {
            path: path.to_string(),
            root: root.to_string(),
            name: "Fixture".to_string(),
            kind: Kind::Sample,
            category: "Other".to_string(),
            tags: vec!["Original".to_string()],
            pack: "Fixture".to_string(),
            bpm: None,
            key: None,
            size,
            modified,
            metadata_source: "fixture".to_string(),
            preview_path: None,
            duration: None,
            duration_checked: false,
            kit_sound_count: None,
            kit_metadata_error: None,
        }
    }

    /// Deterministic interleavings of a header read finishing around a user edit and around a
    /// changed file (`metadataCompletionSelfTest`).
    #[test]
    fn header_lengths_land_on_the_latest_labels_of_the_same_file() {
        let folder = std::env::temp_dir().join(format!("st-library-completion-{}-{}", std::process::id(), util::unique_id()));
        let library = Library::open(LibraryConfig::isolated(folder.join("Cache"), Vec::new())).unwrap();
        // Hold the real reader while completions are driven by hand.
        let release = library.hold_metadata_worker();
        let root = folder.to_string_lossy().into_owned();
        let path = folder.join("Fixture.wav").to_string_lossy().into_owned();
        let result = |library: &Library| library.search(SearchRequest::default()).unwrap().items.remove(0);
        library.set_item_for_test(fixture(&path, &root, 10, 1.0));
        assert_eq!(result(&library).duration_status, "pending", "the visible row queues its header");
        library.edit_metadata(serde_json::from_value(json!({ "paths": [path], "changes": { "tags": ["Edited"] } })).unwrap()).unwrap();
        assert_eq!(result(&library).duration_status, "pending", "the header stays pending after an interleaved edit");
        library.complete_duration_for_test(&path, 10, 1.0, Some(0.5));
        let finished = result(&library);
        assert_eq!((finished.duration, finished.duration_status.as_str(), finished.tags.clone()), (Some(0.5), "known", vec!["Edited".to_string()]), "a completion for the old item keeps newer labels");

        library.set_item_for_test(fixture(&path, &root, 20, 2.0));
        result(&library);
        library.complete_duration_for_test(&path, 10, 1.0, Some(99.0));
        assert!(result(&library).duration.is_none() && library.queued_durations_for_test().contains(&path), "a changed file is queued again instead of taking an obsolete length");
        library.complete_duration_for_test(&path, 20, 2.0, Some(0.75));
        let updated = result(&library);
        assert_eq!((updated.duration, updated.tags), (Some(0.75), vec!["Edited".to_string()]));
        drop(release);
        drop(library);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_service_is_shareable_between_threads() {
        fn shareable<T: Send + Sync + 'static>() {}
        shareable::<Library>();
    }

    #[test]
    fn probing_a_folder() {
        let folder = std::env::temp_dir().join(format!("st-library-probe-root-{}-{}", std::process::id(), util::unique_id()));
        fs::create_dir_all(folder.join("Sub")).unwrap();
        fs::write(folder.join("a.wav"), [1]).unwrap();
        fs::write(folder.join(".hidden"), [1]).unwrap();
        let probe = probe_root(&folder.to_string_lossy());
        assert!(probe.available && probe.can_read_directory && probe.entry_count == 3);
        assert_eq!(probe.sample_entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["Sub", "a.wav"]);
        assert!(probe.sample_entries[0].attributes.is_directory && probe.sample_entries[1].attributes.is_regular_file && probe.sample_entries[1].attributes.is_readable);
        let value = serde_json::to_value(&probe).unwrap();
        assert_eq!(value["sampleEntries"][1], json!({ "name": "a.wav", "isDirectory": false, "isRegularFile": true, "isSymbolicLink": false, "isHidden": false, "isReadable": true }));
        let _ = fs::remove_dir_all(&folder);
    }
}

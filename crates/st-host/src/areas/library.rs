//! The library area: the sound library behind the Browser workspace (`st-library`).
//!
//! [`install`] opens the [`Library`], forwards its events to the interface and registers its
//! commands. Every command is one method of the library; the two that need something from
//! another area go through [`LibraryArea`].
//!
//! | Command | Lane | Notes |
//! |---|---|---|
//! | `library.status`, `.scan`, `.cancel`, `.favorite`, `.state.set`, `.nks.status`, `.nks.rescan` | quick | they change or read memory and hand file work to the library's own threads |
//! | `library.roots`, `.search`, `.removeRoot`, `.diagnostics`, `.metadata.edit`, `.state.get`, `.nks`, `.nks.browse`, `.nks.artwork`, `.kits.browse`, `.kit`, `kit.read` | blocking | they search the index or touch the disk (a folder on a sleeping drive can take seconds) |
//! | `kit.detail` | blocking | [`LibraryArea::kit_detail`]: adds the installed plug-ins |
//! | `kit.save` | blocking | [`LibraryArea::kit_save`]: adds the session's file and the audio preview |
//! | `library.chooseRoot` | blocking | registered by the session (`st_host::session`): the folder chooser, then `library.scan` |
//!
//! Events (from the library's worker threads): `library.progress`, `library.results`,
//! `library.metadata`, `library.favorite`, `library.error`, `library.kits.progress`,
//! `library.nks.progress`.
//!
//! # Files
//!
//! The library keeps its files in `Library/` in the host's support folder
//! ([`LibraryOptions::for_support`]). Kits are saved to `Documents/SuperTracker/Kits` in the home
//! folder, or to `Kits/` in the support folder when that is not the user's own
//! (`SUPERTRACKER_SUPPORT_DIR`, tests): a private run never writes into the user's kits.
//!
//! # Seams to other areas
//!
//! They are calls through the command table, so each starts to work the day its command is
//! registered, and nothing here changes:
//!
//! | Need | Call | Until it is registered |
//! |---|---|---|
//! | which plug-ins are installed, for `kit.detail`'s `missingPlugins` | `plugins.list` (`st-plugin-*`): the `id` of every entry is a component | plug-in availability is not judged (no warning, `missingPlugins: []`) |
//! | the audio preview a kit is saved with | `audio.kitPreview {groupID, path}` (`st-audio`, export) renders it into a temporary folder; `kit.save` copies it into the package | kits are saved without a preview; a preview the page passes as `previewPath` is still used |
//! | the file to audition for a kit in the browser | [`kit_preview_path`], for the preview player of `sample.preview` | |
//! | `plugin.loadNKS` | [`LibraryArea::library`] and `Library::nks_preset(path)` give the plug-in identity, state and pages of a preset | |

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use st_library::api::{Empty, KitDetail, KitDetailRequest, KitSaveRequest, KitSaveResponse};
use st_library::{Library, LibraryConfig};

use crate::bridge::{CallError, Calls, EventSink};
use crate::commands::Commands;
use crate::session::Session;
use crate::{Error, Result};

/// How [`crate::install`] sets the library up.
#[derive(Debug, Clone)]
pub struct LibraryOptions {
    /// The library's configuration: where its files live, the kit folder, the folders it starts with.
    pub config: LibraryConfig,
}

impl LibraryOptions {
    /// The library of the host whose support folder is `support`.
    ///
    /// - The user's own support folder: kits in `Documents/SuperTracker/Kits`, and the library
    ///   starts with the platform's standard content folders.
    /// - A support folder named by `SUPERTRACKER_SUPPORT_DIR` (a development session): the same
    ///   content folders, but its own index, favorites, labels and kits (`Kits/` in that folder).
    /// - Any other folder (a test, an embedded host): `Kits/` in that folder is the kit folder and
    ///   the only library root; the system's content is not looked at.
    pub fn for_support(support: &Path) -> LibraryOptions {
        let overridden = std::env::var_os("SUPERTRACKER_SUPPORT_DIR").is_some();
        let own = crate::support_dir().is_ok_and(|folder| folder == support);
        let mut config = LibraryConfig::new(support.join("Library"));
        if overridden || !own {
            config.user_kits_dir = Some(support.join("Kits"));
        }
        if !own {
            config.default_roots = Some(config.user_kits_dir.iter().cloned().collect());
        }
        LibraryOptions { config }
    }
}

/// The audio file to play when a kit is auditioned in the browser (a preview stored in or next
/// to the kit), if it has one. The seam for the preview player (`sample.preview` with a kit's
/// path).
pub fn kit_preview_path(kit: &str) -> Option<String> {
    st_library::kit::preview_path(kit)
}

/// The library service of the host.
pub struct LibraryArea {
    library: Arc<Library>,
    calls: Calls,
    session: Arc<Session>,
}

impl LibraryArea {
    /// The library, for other areas (`plugin.loadNKS` reads presets with `Library::nks_preset`).
    pub fn library(&self) -> &Arc<Library> {
        &self.library
    }

    /// Calls a command of another area; `None` while that area has not registered it.
    fn call_if_present(&self, command: &str, payload: Value) -> Result<Option<Value>> {
        match self.calls.call(command, payload) {
            Ok(value) => Ok(Some(value)),
            Err(CallError::NotImplemented(_)) => Ok(None),
            Err(CallError::Failed(message)) => Err(Error::new(message)),
        }
    }

    /// `kit.detail`: the kit's metadata and pads, and what to expect when loading it. The
    /// installed plug-ins come from `plugins.list` (unless the request names
    /// `availableComponents` itself); while no plug-in area is registered, or its list cannot
    /// be read, plug-in availability is not judged.
    fn kit_detail(&self, mut payload: Value) -> Result<KitDetail> {
        if payload.get("availableComponents").is_none_or(Value::is_null) {
            if let Ok(Some(plugins)) = self.call_if_present("plugins.list", json!({})) {
                let list = plugins.as_array().or_else(|| plugins.get("plugins").and_then(Value::as_array));
                if let (Some(list), Some(object)) = (list, payload.as_object_mut()) {
                    let components: Vec<&str> = list.iter().filter_map(|plugin| plugin.get("id").and_then(Value::as_str)).collect();
                    object.insert("availableComponents".to_string(), json!(components));
                }
            }
        }
        let request: KitDetailRequest = serde_json::from_value(payload).map_err(|error| Error::new(format!("`kit.detail` was sent with an unusable payload: {error}.")))?;
        Ok(self.library.kit_detail(request)?)
    }

    /// `kit.save`: saves a group of the session as a kit package.
    ///
    /// The project is the one in the payload (the page publishes with plug-in state captured
    /// and sends that project along); without one, the engine's snapshot or the session's
    /// project. `projectPath` (for relative sample paths) is the session's file. Unless the
    /// payload carries a `previewPath` or says `generatePreview: false`, the audio preview is
    /// rendered by `audio.kitPreview` once that command exists; a preview that cannot be
    /// rendered is a warning, not a failure.
    fn kit_save(&self, mut payload: Value) -> Result<Value> {
        if !payload.is_object() {
            return Err(Error::new("Select a kit to save."));
        }
        if payload.get("project").is_none_or(|project| !project.is_object()) {
            let project = match self.call_if_present("engine.snapshot", json!({}))? {
                Some(snapshot) if snapshot.get("tracks").is_some() => Some(snapshot),
                _ => self.session.project().map(|project| project.to_value()),
            };
            payload["project"] = project.ok_or_else(|| Error::new("Select a kit to save."))?;
        }
        if payload.get("projectPath").is_none_or(Value::is_null) {
            if let Some(path) = self.session.path() {
                payload["projectPath"] = json!(path);
            }
        }
        let mut warnings: Vec<String> = Vec::new();
        let mut preview_folder = None;
        let group = payload.get("groupID").and_then(Value::as_str).unwrap_or_default().to_string();
        let wants_preview = payload.get("previewPath").and_then(Value::as_str).is_none_or(str::is_empty) && payload.get("generatePreview") != Some(&json!(false));
        if wants_preview && !group.is_empty() && self.calls.implements("audio.kitPreview") {
            let folder = preview_scratch();
            let file = folder.join("Preview.wav");
            let rendered = std::fs::create_dir_all(&folder).map_err(|error| error.to_string()).and_then(|()| self.calls.call("audio.kitPreview", json!({ "groupID": group, "path": file })).map_err(|error| error.to_string()));
            match rendered {
                Ok(result) => {
                    payload["previewPath"] = json!(file);
                    warnings.extend(result.get("warning").and_then(Value::as_str).map(str::to_string));
                }
                Err(error) => warnings.push(format!("Kit saved without an audio preview: {error}")),
            }
            preview_folder = Some(folder);
        }
        let request: std::result::Result<KitSaveRequest, _> = serde_json::from_value(payload);
        let saved = request.map_err(|error| Error::new(format!("`kit.save` was sent with an unusable payload: {error}."))).and_then(|request| Ok(self.library.kit_save(request)?));
        if let Some(folder) = preview_folder {
            let _ = std::fs::remove_dir_all(folder);
        }
        let saved = saved?;
        let was_saved = matches!(saved, KitSaveResponse::Saved(_));
        let mut result = serde_json::to_value(saved).map_err(|error| Error::new(error.to_string()))?;
        if was_saved && !warnings.is_empty() {
            let mut all: Vec<Value> = result.get("warnings").and_then(Value::as_array).cloned().unwrap_or_default();
            all.extend(warnings.into_iter().map(Value::String));
            result["warnings"] = Value::Array(all);
        }
        Ok(result)
    }

    /// Stops a running scan and writes what the index holds (the host is quitting). The library
    /// writes favorites, labels and the browser's state when they change; this secures the index
    /// cache, which otherwise waits for a quiet moment.
    pub fn shutdown(&self) {
        let scanning = self.library.status(Empty).is_ok_and(|status| status.scanning);
        if scanning {
            let _ = self.library.cancel(Empty);
        }
        self.library.flush_cache();
    }
}

/// A new folder name for one kit preview render.
fn preview_scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("SuperTracker-KitPreview-{}-{}-{nanos}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)))
}

/// Opens the library, forwards its events and registers the `library.*` and `kit.*` commands.
pub fn install(commands: &mut Commands, session: &Arc<Session>, calls: Calls, events: EventSink, options: &LibraryOptions) -> Result<Arc<LibraryArea>> {
    // The kit folder is a library root from the first launch on (kits are saved into it later).
    if let Some(kits) = &options.config.user_kits_dir {
        let _ = std::fs::create_dir_all(kits);
    }
    let library = Arc::new(Library::open(options.config.clone())?);
    library.set_event_sink(move |name, payload| events.emit(name, payload));
    commands
        .with(&library)
        .register("library.status", Library::status)
        .register("library.scan", Library::scan)
        .register("library.cancel", Library::cancel)
        .register("library.favorite", Library::favorite)
        .register("library.state.set", Library::state_set)
        .register("library.nks.status", Library::nks_status)
        .register("library.nks.rescan", Library::nks_rescan)
        .register_blocking("library.roots", Library::roots)
        .register_blocking("library.search", Library::search)
        .register_blocking("library.removeRoot", Library::remove_root)
        .register_blocking("library.diagnostics", Library::diagnostics)
        .register_blocking("library.metadata.edit", Library::edit_metadata)
        .register_blocking("library.state.get", Library::state_get)
        .register_blocking("library.nks", Library::nks_metadata)
        .register_blocking("library.nks.browse", Library::nks_browse)
        .register_blocking("library.nks.artwork", Library::nks_artwork)
        .register_blocking("library.kits.browse", Library::kits_browse)
        .register_blocking("library.kit", Library::kit_metadata)
        .register_blocking("kit.read", Library::kit_read);
    let area = Arc::new(LibraryArea { library, calls, session: session.clone() });
    commands.with(&area).register_blocking("kit.detail", LibraryArea::kit_detail).register_blocking("kit.save", LibraryArea::kit_save);
    Ok(area)
}

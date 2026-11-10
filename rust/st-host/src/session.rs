//! The session: the project the host holds for the interface, and the commands that publish, open,
//! save and recover it (`engine.project`, `project.*`) and choose files (`dialog.*`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::bridge::{CallError, Calls, EventSink};
use crate::dialogs::{Dialogs, FileDialog, FileFilter};
use crate::document::Document;
use crate::{Commands, Error, NoPayload, Result};

/// The audio side of a project publication. `st-audio` implements this and attaches itself with
/// [`Session::attach_audio`]; until then publications only update the session.
pub trait ProjectAudio: Send + Sync + 'static {
    /// Builds the audio graph for `project`. The returned value is the result of `engine.project`.
    /// Called before the session accepts the project: an error leaves the session as it was.
    fn prepare(&self, project: &Value, options: &PublishOptions) -> Result<Value>;
}

/// How a project is being published.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishOptions {
    /// The project replaces the session (opened, recovered) instead of editing it.
    pub replace: bool,
    /// Restore plug-in state from the project even for instances that already run.
    pub force_plugin_state: bool,
    /// Answer only when the project's plug-ins have been instantiated.
    pub await_plugins: bool,
}

/// The session service. Created by [`crate::install`].
pub struct Session {
    document: Mutex<Box<dyn Document>>,
    dialogs: Arc<dyn Dialogs>,
    calls: Calls,
    events: EventSink,
    audio: OnceLock<Arc<dyn ProjectAudio>>,
    /// Saves, opens and session replacements in flight; the recovery copy waits for them.
    foreground: AtomicUsize,
    startup: Mutex<Startup>,
    autosave_timer: Mutex<Option<mpsc::Sender<()>>>,
}

#[derive(Default)]
struct Startup {
    warnings: Vec<String>,
    open_request: Option<PathBuf>,
}

struct Foreground<'a>(&'a AtomicUsize);

impl Drop for Foreground<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PublishPayload {
    pub project: Option<Value>,
    pub replace: bool,
    pub force_plugin_state: bool,
    pub await_plugins: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NewPayload {
    #[serde(rename = "templateID")]
    pub template_id: Option<String>,
    pub blank: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct OpenPayload {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SavePayload {
    pub project: Option<Value>,
    pub path: Option<PathBuf>,
    pub collect_samples: bool,
}

impl Default for SavePayload {
    fn default() -> Self {
        SavePayload { project: None, path: None, collect_samples: true }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AutosavePayload {
    pub project: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExportDialogPayload {
    /// `"stems"` chooses a folder and reserves a new stems folder in it; anything else names a file.
    pub mode: Option<String>,
    /// The name to propose; the session's project name when absent.
    pub name: Option<String>,
}

fn cancelled() -> Value {
    json!({ "cancelled": true })
}

fn project_filters() -> Vec<FileFilter> {
    vec![FileFilter::new("SuperTracker projects", &["supertracker", "json"])]
}

fn absolute(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(Error::new(format!("File paths must be absolute (got “{}”).", path.display())))
    }
}

impl Session {
    pub(crate) fn new(mut document: Box<dyn Document>, dialogs: Arc<dyn Dialogs>, calls: Calls, events: EventSink) -> Arc<Self> {
        let mut startup = Startup::default();
        if let Err(error) = document.startup() {
            startup.warnings.push(format!("Could not restore the previous session: {error} Opening a blank session."));
        }
        Arc::new(Session {
            document: Mutex::new(document),
            dialogs,
            calls,
            events,
            audio: OnceLock::new(),
            foreground: AtomicUsize::new(0),
            startup: Mutex::new(startup),
            autosave_timer: Mutex::new(None),
        })
    }

    pub(crate) fn register(self: &Arc<Self>, commands: &mut Commands) {
        commands
            .with(self)
            .register_blocking("engine.project", Session::publish)
            .register_blocking("project.new", Session::new_project)
            .register_blocking("project.open", Session::open)
            .register_blocking("project.save", |session: &Session, payload| session.save(payload, false))
            .register_blocking("project.saveAs", |session: &Session, payload| session.save(payload, true))
            .register_blocking("project.autosave", Session::autosave)
            .register_blocking("dialog.openSample", Session::choose_sample)
            .register_blocking("dialog.export", Session::choose_export)
            .register_blocking("project.media.chooseRoot", Session::choose_media_root)
            .register_blocking("library.chooseRoot", Session::choose_library_root);
    }

    /// Connects the audio engine to project publications. Only the first call has an effect.
    pub fn attach_audio(&self, audio: Arc<dyn ProjectAudio>) {
        let _ = self.audio.set(audio);
    }

    fn document(&self) -> MutexGuard<'_, Box<dyn Document>> {
        self.document.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn startup(&self) -> MutexGuard<'_, Startup> {
        self.startup.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn foreground(&self) -> Foreground<'_> {
        self.foreground.fetch_add(1, Ordering::SeqCst);
        Foreground(&self.foreground)
    }

    /// The project as the host holds it.
    pub fn project(&self) -> Option<Value> {
        self.document().project().cloned()
    }

    /// The file the session belongs to.
    pub fn path(&self) -> Option<PathBuf> {
        self.document().path().map(Path::to_path_buf)
    }

    /// Whether the session has changes its file does not.
    pub fn dirty(&self) -> bool {
        self.document().dirty()
    }

    /// Problems found while starting (an unreadable recovery copy), for the handshake to report.
    pub(crate) fn startup_warnings(&self) -> Vec<String> {
        self.startup().warnings.clone()
    }

    /// The operating system asked to open a project file (a double click, a drop on the icon, a
    /// command-line argument). The interface decides what happens to unsaved work: a ready interface
    /// is told with the `project.openRequested` event, and one that is still loading finds the path
    /// in the result of `ready`.
    pub fn request_open(&self, path: PathBuf) {
        if self.events.has_ready_interface() {
            self.events.emit("project.openRequested", json!({ "path": path }));
        } else {
            self.startup().open_request = Some(path);
        }
    }

    pub(crate) fn take_open_request(&self) -> Option<PathBuf> {
        self.startup().open_request.take()
    }

    /// Calls a command of another area, treating "not implemented yet" as "that area has nothing to do".
    fn call_if_present(&self, command: &str, payload: Value) -> Result<Option<Value>> {
        match self.calls.call(command, payload) {
            Ok(value) => Ok(Some(value)),
            Err(CallError::NotImplemented(_)) => Ok(None),
            Err(CallError::Failed(message)) => Err(Error::new(message)),
        }
    }

    pub(crate) fn prepare_audio(&self, project: &Value, options: PublishOptions) -> Result<Option<Value>> {
        self.audio.get().map(|audio| audio.prepare(project, &options)).transpose()
    }

    /// Stops sound and disarms recording before the session is replaced.
    fn quiet_audio(&self) -> Result<()> {
        self.call_if_present("engine.panic", json!({}))?;
        self.call_if_present("transport.recordArm", json!({ "armed": false }))?;
        Ok(())
    }

    /// The state of every plug-in instance, as the audio engine reports it in `engine.snapshot`.
    /// `None` while no audio engine is attached: the project then keeps the states it carries.
    fn captured_plugin_states(&self) -> Result<Option<Value>> {
        let Some(snapshot) = self.call_if_present("engine.snapshot", json!({}))? else {
            return Ok(None);
        };
        let Some(plugins) = snapshot.get("audioPlugins").and_then(Value::as_array) else {
            return Err(Error::new("The audio engine returned no plug-in state."));
        };
        for plugin in plugins {
            if let Some(problem) = plugin.get("stateError").and_then(Value::as_str) {
                let name = plugin.get("name").and_then(Value::as_str).unwrap_or("A plug-in");
                return Err(Error::new(format!("{name}: {problem}")));
            }
        }
        Ok(Some(Value::Array(plugins.clone())))
    }

    /// Stores freshly captured plug-in states in the session's project. Fails when the project
    /// changed while they were being captured: the states would belong to an older project.
    fn store_plugin_states(&self, before: &Option<Value>, states: Option<Value>) -> Result<()> {
        let Some(states) = states else { return Ok(()) };
        let mut document = self.document();
        if document.project() != before.as_ref() {
            return Err(Error::new("The project changed while capturing plug-in state. Retry after the current edit finishes."));
        }
        let Some(mut project) = document.project().cloned() else { return Ok(()) };
        if project.get("audioPlugins") == Some(&states) {
            return Ok(());
        }
        project["audioPlugins"] = states;
        document.update(project, true)
    }

    /// `engine.project`: the interface publishes its project.
    pub fn publish(&self, payload: PublishPayload) -> Result<Value> {
        let Some(project) = payload.project else {
            return Err(Error::new("`engine.project` needs the project to publish."));
        };
        self.document().validate(&project)?;
        let _foreground = self.foreground();
        let options = PublishOptions {
            replace: payload.replace,
            force_plugin_state: payload.force_plugin_state || payload.replace,
            await_plugins: payload.await_plugins,
        };
        let result = self.prepare_audio(&project, options)?;
        self.document().update(project, true)?;
        Ok(result.unwrap_or_else(|| json!({ "audio": false })))
    }

    /// `project.new`: replaces the session with an untitled one.
    pub fn new_project(&self, payload: NewPayload) -> Result<Value> {
        if !payload.blank && payload.template_id.is_some() {
            return Err(Error::new("Project templates are not available yet: `project.templates.*` is not implemented in the Rust host."));
        }
        let _foreground = self.foreground();
        self.document().autosave()?;
        self.quiet_audio()?;
        self.document().start_new(None)?;
        // Null: the interface starts from its own blank project.
        Ok(Value::Null)
    }

    /// `project.open`: opens the file at `path`, or the one chosen in the Open window.
    pub fn open(&self, payload: OpenPayload) -> Result<Value> {
        let chosen = match payload.path {
            Some(path) => absolute(path)?,
            None => match self.dialogs.choose(&FileDialog::open_file("Open Project", project_filters()))? {
                Some(path) => path,
                None => return Ok(cancelled()),
            },
        };
        let _foreground = self.foreground();
        self.document().autosave()?;
        let project = self.document().read(&chosen)?;
        self.prepare_audio(&project, PublishOptions { replace: true, force_plugin_state: true, await_plugins: false })?;
        self.call_if_present("transport.recordArm", json!({ "armed": false }))?;
        self.document().accept_opened(project.clone(), &chosen);
        Ok(json!({ "project": project, "path": chosen, "dirty": false }))
    }

    /// `project.save` and `project.saveAs`.
    pub fn save(&self, payload: SavePayload, save_as: bool) -> Result<Value> {
        let _foreground = self.foreground();
        if let Some(project) = payload.project {
            self.document().update(project, true)?;
        }
        let known = if save_as { None } else { self.path() };
        let destination = match payload.path.map(absolute).transpose()?.or(known) {
            Some(path) => path,
            None => {
                let name = self.project_name().unwrap_or_else(|| "Untitled".to_string());
                let dialog = FileDialog::save_file("Save Project", &format!("{name}.supertracker"), project_filters());
                match self.dialogs.choose(&dialog)? {
                    Some(path) if path.extension().is_none() => path.with_extension("supertracker"),
                    Some(path) => path,
                    None => return Ok(cancelled()),
                }
            }
        };
        let before = self.project();
        let states = self.captured_plugin_states()?;
        self.store_plugin_states(&before, states)?;
        let saved = self.document().save(&destination, payload.collect_samples)?;
        let mut result = json!({ "project": saved, "path": destination, "saved": true });
        // The file is written. A failing recovery copy is reported, but does not undo the save.
        if let Err(error) = self.document().autosave() {
            result["recoveryWarning"] = json!(error.message());
        }
        Ok(result)
    }

    /// `project.autosave`: writes the recovery copy, unless a save or session change is in flight.
    pub fn autosave(&self, payload: AutosavePayload) -> Result<Value> {
        if self.foreground.load(Ordering::SeqCst) > 0 {
            return Ok(json!({ "saved": false, "deferred": true }));
        }
        if let Some(project) = payload.project {
            self.document().update(project, true)?;
        }
        self.write_recovery()?;
        Ok(json!({ "saved": true }))
    }

    /// Writes the recovery copy now, whatever else is going on (the host is about to stop).
    pub fn save_recovery(&self) -> Result<()> {
        let _foreground = self.foreground();
        self.write_recovery()
    }

    /// Captures plug-in state and writes the recovery copy.
    pub(crate) fn write_recovery(&self) -> Result<()> {
        let before = self.project();
        if before.is_none() {
            return Ok(());
        }
        let states = self.captured_plugin_states()?;
        self.store_plugin_states(&before, states)?;
        self.document().autosave()
    }

    /// Accepts the interface's final project (if any) and writes the recovery copy before quitting.
    pub(crate) fn secure_for_quit(&self, project: Option<Value>) -> Result<()> {
        let _foreground = self.foreground();
        if let Some(project) = project {
            self.document().update(project, true)?;
        }
        self.write_recovery()
    }

    /// Writes the recovery copy every `interval` while the host runs, like the interface's own
    /// `project.autosave` but independent of it. A failure is reported to the interface as an
    /// `error` event. The timer ends when the session is dropped or [`Session::stop`] is called.
    pub fn start_autosave_timer(self: &Arc<Self>, interval: Duration) {
        let (stop, stopped) = mpsc::channel::<()>();
        *self.autosave_timer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(stop);
        let session = Arc::downgrade(self);
        let spawned = std::thread::Builder::new().name("st-host autosave".to_string()).spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(interval) {
                let Some(session) = session.upgrade() else { break };
                if session.foreground.load(Ordering::SeqCst) > 0 {
                    continue;
                }
                if let Err(error) = session.write_recovery() {
                    session.events.emit("error", json!({ "message": format!("Recovery save failed: {error}") }));
                }
            }
        });
        if let Err(error) = spawned {
            eprintln!("st-host: the recovery timer could not start: {error}");
        }
    }

    /// Ends the recovery timer.
    pub fn stop(&self) {
        self.autosave_timer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    }

    fn project_name(&self) -> Option<String> {
        let document = self.document();
        let name = document.project()?.get("name")?.as_str()?.trim();
        (!name.is_empty()).then(|| name.replace(['/', ':'], "-"))
    }

    /// `dialog.openSample`: an audio file to load.
    pub fn choose_sample(&self, _: NoPayload) -> Result<Value> {
        let audio = FileFilter::new("Audio files", &["wav", "wave", "aif", "aiff", "aifc", "caf", "flac", "mp3", "m4a", "ogg"]);
        Ok(match self.dialogs.choose(&FileDialog::open_file("Choose a Sample", vec![audio]))? {
            Some(path) => json!({ "path": path }),
            None => cancelled(),
        })
    }

    /// `dialog.export`: where to write a mixdown (a file) or stems (a new folder).
    pub fn choose_export(&self, payload: ExportDialogPayload) -> Result<Value> {
        let name = payload.name.filter(|name| !name.trim().is_empty()).or_else(|| self.project_name()).unwrap_or_else(|| "Mixdown".to_string());
        if payload.mode.as_deref() == Some("stems") {
            let dialog = FileDialog::choose_folder("A new folder will be created here for this stem export.").confirm_label("Export here");
            let Some(parent) = self.dialogs.choose(&dialog)? else { return Ok(cancelled()) };
            let folder = reserve_stems_folder(&parent, &name)?;
            return Ok(json!({ "path": folder, "mode": "stems" }));
        }
        let dialog = FileDialog::save_file("Export Audio", &format!("{name}.wav"), vec![FileFilter::new("WAV audio", &["wav"])]);
        Ok(match self.dialogs.choose(&dialog)? {
            Some(path) => json!({ "path": path }),
            None => cancelled(),
        })
    }

    /// `project.media.chooseRoot`: a folder to search for missing media.
    pub fn choose_media_root(&self, _: NoPayload) -> Result<Value> {
        Ok(match self.dialogs.choose(&FileDialog::choose_folder("Choose a Folder to Search"))? {
            Some(path) => json!({ "path": path }),
            None => cancelled(),
        })
    }

    /// `library.chooseRoot`: a folder to add to the sound library; its scan is the library's
    /// (`library.scan`), whose result is this command's result.
    pub fn choose_library_root(&self, _: NoPayload) -> Result<Value> {
        let Some(folder) = self.dialogs.choose(&FileDialog::choose_folder("Add a Folder to the Library"))? else {
            return Ok(cancelled());
        };
        self.calls.call("library.scan", json!({ "roots": [folder] })).map_err(|error| Error::new(error.to_string()))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Creates a new folder "`<name>` stems" in `parent` ("… stems 2", "… stems 3" when taken), so an
/// earlier export or an unrelated folder is never reused because it has the same name.
pub fn reserve_stems_folder(parent: &Path, project_name: &str) -> Result<PathBuf> {
    if !parent.is_dir() {
        return Err(Error::new("Choose an existing folder for the stem export."));
    }
    let cleaned: String = project_name.chars().map(|character| if character.is_control() || character == '/' || character == ':' { '-' } else { character }).collect();
    let cleaned = cleaned.trim_matches(|character: char| character.is_whitespace() || character == '.');
    let mut name = String::new();
    for character in cleaned.chars() {
        if name.len() + character.len_utf8() > 160 {
            break;
        }
        name.push(character);
    }
    if name.is_empty() {
        name.push_str("Untitled");
    }
    let base = format!("{name} stems");
    for attempt in 0..10_000 {
        let candidate = parent.join(if attempt == 0 { base.clone() } else { format!("{base} {}", attempt + 1) });
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::new(format!("Could not create a folder for the stems in {}: {error}.", parent.display()))),
        }
    }
    Err(Error::new("Could not reserve a new stem folder. Choose another destination."))
}

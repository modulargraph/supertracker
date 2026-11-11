//! The session: the project the host holds for the interface, and the commands that publish, open,
//! save and recover it (`engine.project`, `project.*`) and choose files (`dialog.*`).
//!
//! The document itself (validation, project files and their media folder, the recovery copy,
//! templates, MIDI files) is `st-project`'s; this module is the bridge side of it. Errors from
//! `st-project` reach the interface with their text unchanged, including the one that refuses a
//! project of another format version.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use st_project::session::Session as Document;
use st_project::Project;

use crate::bridge::{CallError, Calls, EventSink};
use crate::dialogs::{Dialogs, FileDialog, FileFilter};
use crate::{Commands, Error, NoPayload, Result};

/// The audio side of a project publication. `st-audio` implements this and attaches itself with
/// [`Session::attach_audio`]; until then publications only update the session.
pub trait ProjectAudio: Send + Sync + 'static {
    /// Builds the audio graph for `project`. The returned value is the result of `engine.project`.
    /// Called before the session accepts the project: an error leaves the session as it was.
    /// With `replace` the project replaces the session (opened, recovered, restored) instead of
    /// editing it: plug-in state is loaded from the project even into instances that already run.
    fn prepare(&self, project: &Project, replace: bool) -> Result<Value>;
}

/// The session service. Created by [`crate::install`].
pub struct Session {
    document: Mutex<Document>,
    dialogs: Arc<dyn Dialogs>,
    calls: Calls,
    events: EventSink,
    audio: OnceLock<Arc<dyn ProjectAudio>>,
    /// Saves, opens and session replacements in flight; the recovery copy waits for them.
    foreground: AtomicUsize,
    startup: Mutex<Startup>,
    autosave_timer: Mutex<Option<mpsc::Sender<()>>>,
    /// Told about every project the audio side accepted ([`Session::observe_projects`]).
    project_observers: Mutex<Vec<ProjectObserver>>,
}

type ProjectObserver = Arc<dyn Fn(&Project) + Send + Sync>;

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
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NewPayload {
    #[serde(rename = "templateID")]
    pub template_id: Option<String>,
    pub blank: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PathPayload {
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
pub struct ProjectPayload {
    pub project: Option<Value>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveTemplatePayload {
    pub project: Option<Value>,
    pub name: Option<String>,
    pub id: Option<String>,
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

fn midi_filters() -> Vec<FileFilter> {
    vec![FileFilter::new("MIDI files", &["mid", "midi"])]
}

pub(crate) fn absolute(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(Error::new(format!("File paths must be absolute (got “{}”).", path.display())))
    }
}

/// A name that can be part of a file name.
fn file_stem(name: &str) -> String {
    name.trim().replace(['/', ':'], "-")
}

impl Session {
    pub(crate) fn new(support: &Path, dialogs: Arc<dyn Dialogs>, calls: Calls, events: EventSink) -> Result<Arc<Self>> {
        let mut document = Document::new(support)?;
        let mut startup = Startup::default();
        // An unreadable recovery copy or default template does not stop the launch; it is reported.
        startup.warnings.extend(document.startup()?.warning);
        Ok(Arc::new(Session {
            document: Mutex::new(document),
            dialogs,
            calls,
            events,
            audio: OnceLock::new(),
            foreground: AtomicUsize::new(0),
            startup: Mutex::new(startup),
            autosave_timer: Mutex::new(None),
            project_observers: Mutex::new(Vec::new()),
        }))
    }

    pub(crate) fn register(self: &Arc<Self>, commands: &mut Commands) {
        commands
            .with(self)
            .register_blocking("engine.project", Session::publish)
            .register("routing.validate", Session::validate_routing)
            .register_blocking("routing.apply", Session::apply_routing)
            .register_blocking("project.new", Session::new_project)
            .register_blocking("project.open", Session::open)
            .register_blocking("project.save", |session: &Session, payload| session.save(payload, false))
            .register_blocking("project.saveAs", |session: &Session, payload| session.save(payload, true))
            .register_blocking("project.autosave", Session::autosave)
            .register_blocking("project.importMIDI", Session::import_midi)
            .register_blocking("project.exportMIDI", Session::export_midi)
            .register_blocking("project.templates.list", Session::list_templates)
            .register_blocking("project.templates.save", Session::save_template)
            .register_blocking("project.templates.setDefault", Session::set_default_template)
            .register_blocking("dialog.openSample", Session::choose_sample)
            .register_blocking("dialog.export", Session::choose_export)
            .register_blocking("project.media.chooseRoot", Session::choose_media_root)
            .register_blocking("library.chooseRoot", Session::choose_library_root);
    }

    /// Connects the audio engine to project publications. Only the first call has an effect.
    pub fn attach_audio(&self, audio: Arc<dyn ProjectAudio>) {
        let _ = self.audio.set(audio);
    }

    fn document(&self) -> MutexGuard<'_, Document> {
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
    pub fn project(&self) -> Option<Project> {
        self.document().project().cloned()
    }

    /// The file the session belongs to.
    pub fn path(&self) -> Option<PathBuf> {
        self.document().path().map(Path::to_path_buf)
    }

    /// Whether the session has changes its file does not.
    pub fn dirty(&self) -> bool {
        self.document().is_dirty()
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

    /// The project a command works on: the one in its payload, or the session's.
    pub(crate) fn candidate(&self, supplied: Option<Value>, missing: &str) -> Result<Project> {
        let project = match supplied {
            Some(value) => Project::from_value(value)?,
            None => self.project().ok_or_else(|| Error::new(missing))?,
        };
        st_project::validate::validate(&project)?;
        Ok(project)
    }

    /// Calls a command of another area, treating "not implemented yet" as "that area has nothing to do".
    fn call_if_present(&self, command: &str, payload: Value) -> Result<Option<Value>> {
        match self.calls.call(command, payload) {
            Ok(value) => Ok(Some(value)),
            Err(CallError::NotImplemented(_)) => Ok(None),
            Err(CallError::Failed(message)) => Err(Error::new(message)),
        }
    }

    pub(crate) fn prepare_audio(&self, project: &Project, replace: bool) -> Result<Option<Value>> {
        let result = self.audio.get().map(|audio| audio.prepare(project, replace)).transpose()?;
        let observers = self.project_observers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        for observer in observers {
            observer(project);
        }
        Ok(result)
    }

    /// Calls `observer` with every project that is published, opened or restored, right after the
    /// audio side has accepted it (so on the thread of that command). For the areas that follow
    /// the project without being part of the audio engine: MIDI routes read the instruments'
    /// devices and channels from it.
    pub fn observe_projects(&self, observer: impl Fn(&Project) + Send + Sync + 'static) {
        self.project_observers.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(Arc::new(observer));
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

    /// Captures the plug-in states and stores them in the session's project. Fails when the project
    /// changed while they were being captured: the states would belong to an older project.
    fn capture_plugin_states(&self) -> Result<()> {
        let before = self.project();
        let (Some(held), Some(states)) = (&before, self.captured_plugin_states()?) else { return Ok(()) };
        let mut document = self.document();
        if document.project() != before.as_ref() {
            return Err(Error::new("The project changed while capturing plug-in state. Retry after the current edit finishes."));
        }
        let mut value = held.to_value();
        if value.get("audioPlugins") == Some(&states) {
            return Ok(());
        }
        value["audioPlugins"] = states;
        document.update(Project::from_value(value)?, true)?;
        Ok(())
    }

    /// `engine.project`: the interface publishes its project.
    pub fn publish(&self, payload: PublishPayload) -> Result<Value> {
        let Some(project) = payload.project else {
            return Err(Error::new("`engine.project` needs the project to publish."));
        };
        let project = Project::from_value(project)?;
        st_project::validate::validate(&project)?;
        let _foreground = self.foreground();
        let result = self.prepare_audio(&project, payload.replace)?;
        self.document().update(project, true)?;
        Ok(result.unwrap_or_else(|| json!({ "audio": false })))
    }

    /// `routing.validate`: whether the host would accept this project. The interface asks before
    /// it commits a change to buses, outputs, sends or sidechains.
    pub fn validate_routing(&self, payload: PublishPayload) -> Result<Value> {
        let Some(project) = payload.project else {
            return Err(Error::new("`routing.validate` needs the project to check."));
        };
        let problem = match Project::from_value(project) {
            Ok(project) => st_project::validate::validate(&project).err().map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        };
        Ok(match problem {
            None => json!({ "valid": true }),
            Some(error) => json!({ "valid": false, "error": error }),
        })
    }

    /// `routing.apply`: publishes a project whose routing changed and answers with the project
    /// as the session holds it, what the rebuilt mixer offers and what it could not connect.
    pub fn apply_routing(&self, payload: PublishPayload) -> Result<Value> {
        self.publish(payload)?;
        let Some(project) = self.project() else {
            return Err(Error::new("The session has no project."));
        };
        let mut result = json!({ "project": project.to_value() });
        if let Some(capabilities) = self.call_if_present("routing.capabilities", json!({}))? {
            result["capabilities"] = capabilities;
        }
        if let Some(status) = self.call_if_present("routing.status", json!({}))? {
            result["status"] = status;
        }
        Ok(result)
    }

    /// `project.new`: replaces the session with an untitled one, from a template (the given one,
    /// else the default one) or, with null as the result, the interface's blank project.
    pub fn new_project(&self, payload: NewPayload) -> Result<Value> {
        // Resolved first: a template that cannot be opened must not cost the open session.
        let proposed = self.document().proposed_new_project(payload.template_id.as_deref(), payload.blank)?;
        let _foreground = self.foreground();
        self.document().autosave()?;
        self.quiet_audio()?;
        let result = match &proposed {
            Some(project) => json!({ "project": project.to_value() }),
            None => Value::Null,
        };
        self.document().accept_new_project(proposed)?;
        Ok(result)
    }

    /// `project.open`: opens the file at `path`, or the one chosen in the Open window.
    pub fn open(&self, payload: PathPayload) -> Result<Value> {
        let chosen = match payload.path {
            Some(path) => absolute(path)?,
            None => match self.dialogs.choose(&FileDialog::open_file("Open Project", project_filters()))? {
                Some(path) => path,
                None => return Ok(cancelled()),
            },
        };
        let _foreground = self.foreground();
        self.document().autosave()?;
        // Read once for the audio engine, which may refuse it, before the session changes.
        let project = st_project::files::read_project(&chosen)?;
        self.prepare_audio(&project, true)?;
        self.call_if_present("transport.recordArm", json!({ "armed": false }))?;
        let mut document = self.document();
        let opened = document.open(&chosen)?.to_value();
        Ok(json!({ "project": opened, "path": chosen, "dirty": document.is_dirty() }))
    }

    /// `project.save` and `project.saveAs`.
    pub fn save(&self, payload: SavePayload, save_as: bool) -> Result<Value> {
        let _foreground = self.foreground();
        if let Some(project) = payload.project {
            self.document().update(Project::from_value(project)?, true)?;
        }
        let known = if save_as { None } else { self.path() };
        let destination = match payload.path.map(absolute).transpose()?.or(known) {
            Some(path) => path,
            None => {
                let name = self.project().map(|project| file_stem(project.display_name())).unwrap_or_else(|| "Untitled".to_string());
                let dialog = FileDialog::save_file("Save Project", &format!("{name}.supertracker"), project_filters());
                match self.dialogs.choose(&dialog)? {
                    Some(path) if path.extension().is_none() => path.with_extension("supertracker"),
                    Some(path) => path,
                    None => return Ok(cancelled()),
                }
            }
        };
        self.capture_plugin_states()?;
        let mut document = self.document();
        let outcome = document.save_with(&destination, None, payload.collect_samples)?;
        let saved = document.project().map(Project::to_value);
        let mut result = json!({ "project": saved, "path": outcome.path, "saved": true });
        // The file is written. A failing recovery copy is reported, but does not undo the save.
        if let Some(warning) = outcome.recovery_warning {
            result["recoveryWarning"] = json!(warning);
        }
        Ok(result)
    }

    /// `project.autosave`: writes the recovery copy, unless a save or session change is in flight.
    pub fn autosave(&self, payload: ProjectPayload) -> Result<Value> {
        if self.foreground.load(Ordering::SeqCst) > 0 {
            return Ok(json!({ "saved": false, "deferred": true }));
        }
        if let Some(project) = payload.project {
            self.document().update(Project::from_value(project)?, true)?;
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
        self.capture_plugin_states()?;
        Ok(self.document().autosave()?)
    }

    /// Accepts the interface's final project (if any) and writes the recovery copy before quitting.
    pub(crate) fn secure_for_quit(&self, project: Option<Value>) -> Result<()> {
        let _foreground = self.foreground();
        if let Some(project) = project {
            self.document().update(Project::from_value(project)?, true)?;
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

    /// `project.importMIDI`: a Standard MIDI File becomes a new, unsaved session.
    pub fn import_midi(&self, payload: PathPayload) -> Result<Value> {
        let chosen = match payload.path {
            Some(path) => absolute(path)?,
            None => match self.dialogs.choose(&FileDialog::open_file("Import MIDI File", midi_filters()))? {
                Some(path) => path,
                None => return Ok(cancelled()),
            },
        };
        let _foreground = self.foreground();
        self.document().autosave()?;
        let project = st_project::midi::import_file(&chosen)?;
        st_project::validate::validate(&project)?;
        self.prepare_audio(&project, true)?;
        self.call_if_present("transport.recordArm", json!({ "armed": false }))?;
        let value = project.to_value();
        let mut document = self.document();
        document.accept_new_project(None)?;
        document.update(project, true)?;
        let warnings = value.pointer("/midiImport/warnings").cloned().unwrap_or_else(|| json!([]));
        Ok(json!({ "project": value, "path": chosen, "warnings": warnings }))
    }

    /// `project.exportMIDI`: writes the project (the payload's, else the session's) as a Standard MIDI File.
    pub fn export_midi(&self, payload: ProjectPayload) -> Result<Value> {
        let project = self.candidate(payload.project, "There is no project to export.")?;
        let chosen = match payload.path {
            Some(path) => absolute(path)?,
            None => {
                let dialog = FileDialog::save_file("Export MIDI File", &format!("{}.mid", file_stem(project.display_name())), midi_filters());
                match self.dialogs.choose(&dialog)? {
                    Some(path) if path.extension().is_none() => path.with_extension("mid"),
                    Some(path) => path,
                    None => return Ok(cancelled()),
                }
            }
        };
        let exported = st_project::midi::export_file(&project, &chosen)?;
        Ok(json!({
            "path": chosen,
            "noteCount": exported.note_count,
            "trackCount": exported.track_count,
            "ticksPerQuarter": exported.ticks_per_quarter,
            "warnings": exported.warnings,
        }))
    }

    /// `project.templates.list`
    pub fn list_templates(&self, _: NoPayload) -> Result<Value> {
        let document = self.document();
        let templates: Vec<Value> =
            document.templates().into_iter().map(|template| json!({ "id": template.id, "name": template.name, "path": template.path })).collect();
        Ok(json!({ "templates": templates, "defaultID": document.default_template_id()? }))
    }

    /// `project.templates.save`: saves the project (the payload's, else the session's) as a template.
    pub fn save_template(&self, payload: SaveTemplatePayload) -> Result<Value> {
        let missing = "Choose a project and name for the template.";
        let name = payload.name.ok_or_else(|| Error::new(missing))?;
        let project = self.candidate(payload.project, missing)?;
        let template = self.document().save_template(&project, &name, payload.id.as_deref())?;
        Ok(json!({ "id": template.id, "name": template.name, "path": template.path }))
    }

    /// `project.templates.setDefault`
    ///
    /// `{id}`: a template's id, or null for a blank default. The field must be present, so that a
    /// request that lost it does not clear the default.
    pub fn set_default_template(&self, payload: Value) -> Result<Value> {
        let id = match payload.get("id") {
            Some(Value::String(id)) => Some(id.as_str()),
            Some(Value::Null) => None,
            _ => return Err(Error::new("Choose a template identifier, or null for a blank default.")),
        };
        let document = self.document();
        document.set_default_template(id)?;
        Ok(json!({ "defaultID": document.default_template_id()? }))
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
        let name = payload
            .name
            .filter(|name| !name.trim().is_empty())
            .or_else(|| self.project().map(|project| project.display_name().to_string()))
            .map(|name| file_stem(&name))
            .unwrap_or_else(|| "Mixdown".to_string());
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
        Ok(self.calls.call("library.scan", json!({ "roots": [folder] }))?)
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

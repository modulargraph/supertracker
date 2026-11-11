//! The application-level commands: the `ready` handshake, `app.external`, the quit handshake
//! (`app.quitAnswer`), and [`install`], which puts everything `st-host` implements into a bridge.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::bridge::{BridgeBuilder, CallError, Calls, EventSink};
use crate::dialogs::Dialogs;
use crate::media::MediaRelink;
use crate::session::Session;
use crate::settings::Settings;
use crate::{Error, NoPayload, Result};

/// What [`install`] needs from the executable.
pub struct HostOptions {
    /// The support folder ([`crate::support_dir`]).
    pub support: PathBuf,
    /// The file choosers of this run.
    pub dialogs: Arc<dyn Dialogs>,
    /// Opens links and reveals files. [`SystemOpener`] outside tests.
    pub opener: Arc<dyn Opener>,
    /// How often the host writes the recovery copy on its own; `None` for never (tests).
    pub autosave_every: Option<Duration>,
    /// The audio engine; `None` for a host without one (tests of the host's own commands).
    pub audio: Option<crate::areas::audio::AudioOptions>,
    /// Plug-in hosting ([`crate::areas::plugins`]), on a host with an audio engine; `None` for a host without it.
    pub plugins: Option<crate::areas::plugins::PluginAreaOptions>,
    /// The library, MIDI, the Maschine MK3 and AI control ([`crate::areas::services`]).
    pub services: crate::areas::services::ServiceOptions,
}

impl HostOptions {
    /// The options of a normal run: the system's opener, recovery every 20 s.
    pub fn new(support: PathBuf, dialogs: Arc<dyn Dialogs>) -> Self {
        let services = crate::areas::services::ServiceOptions::for_support(&support);
        HostOptions { services, support, dialogs, opener: Arc::new(SystemOpener), autosave_every: Some(Duration::from_secs(20)), audio: Some(crate::areas::audio::AudioOptions::from_environment()), plugins: Some(crate::areas::plugins::PluginAreaOptions::from_environment()) }
    }
}

/// The services `st-host` runs, for the executable and for the areas that plug into them.
pub struct Host {
    pub session: Arc<Session>,
    pub settings: Arc<Settings>,
    pub lifecycle: Arc<Lifecycle>,
    /// The audio area, when the host was installed with one.
    pub audio: Option<Arc<crate::areas::audio::Audio>>,
    /// The plug-in area, when the host has an audio engine and plug-in hosting could start.
    pub plugins: Option<Arc<crate::areas::plugins::Plugins>>,
    /// The library, MIDI, the Maschine MK3 and AI control. The executable starts AI control
    /// (`services.start_agent()`) and ends them all (`services.shutdown()`).
    pub services: crate::areas::services::Services,
}

/// Registers every command `st-host` implements and returns its services.
///
/// The executable calls this first, then lets each area register its own commands on the same
/// builder, then starts the bridge.
pub fn install(bridge: &mut BridgeBuilder, options: HostOptions) -> Result<Host> {
    let settings = Arc::new(Settings::open(&options.support)?);
    let session = Session::new(&options.support, options.dialogs, bridge.calls(), bridge.events())?;
    let lifecycle = Arc::new(Lifecycle { events: bridge.events(), session: session.clone(), state: Mutex::new(Quit::default()) });
    let handshake = Arc::new(Handshake { session: session.clone(), settings: settings.clone(), calls: bridge.calls() });
    let external = Arc::new(External { opener: options.opener });

    let events = bridge.events();
    let audio_events = bridge.events();
    let sample_events = bridge.events();
    let plugin_events = bridge.events();
    let (service_events, service_calls) = (bridge.events(), bridge.calls());
    let commands = bridge.commands();
    session.register(commands);
    MediaRelink::register(session.clone(), events, commands);
    commands.with(&handshake).register_blocking("ready", Handshake::ready);
    commands.with(&settings).register("settings.get", Settings::command_get).register("settings.set", Settings::command_set);
    commands.with(&external).register_blocking("app.external", External::open);
    commands.with(&lifecycle).register_blocking("app.quitAnswer", Lifecycle::answer);
    let audio = match &options.audio {
        Some(audio) => Some(crate::areas::audio::install(commands, &session, &settings, audio_events, audio)?),
        None => None,
    };
    if let Some(audio) = &audio {
        crate::areas::samples::install(commands, audio, &options.support, sample_events.clone());
        crate::areas::export::install(commands, audio, &options.support, sample_events.clone());
        crate::areas::recording::install(commands, audio, &options.support, sample_events);
    }
    // Plug-in hosting that cannot start (no place for its sockets) costs the plug-ins, not the host.
    let plugins = match (&audio, &options.plugins) {
        (Some(audio), Some(plugins)) => crate::areas::plugins::install(commands, audio, &settings, &options.support, plugin_events, plugins).map_err(|error| eprintln!("supertracker: {error}")).ok(),
        _ => None,
    };
    let services = crate::areas::services::install(commands, &session, &settings, &lifecycle, service_events, service_calls, audio.as_ref(), &options.support, &options.services)?;

    if let Some(interval) = options.autosave_every {
        session.start_autosave_timer(interval);
    }
    Ok(Host { session, settings, lifecycle, audio, plugins, services })
}

struct Handshake {
    session: Arc<Session>,
    settings: Arc<Settings>,
    calls: Calls,
}

impl Handshake {
    /// What another area contributes to the handshake, or `fallback` while the area is not there.
    fn area_part(&self, command: &str, fallback: Value, problems: &mut Vec<String>) -> Value {
        match self.calls.call(command, json!({})) {
            Ok(value) => value,
            Err(CallError::NotImplemented(_)) => fallback,
            Err(CallError::Failed(message)) => {
                problems.push(message);
                fallback
            }
        }
    }

    /// `ready`: the interface has loaded. The result is the session to show and what the host has:
    ///
    /// - `version`: the host's version; `commands`: every command this host implements;
    /// - `project` (or null for a blank session), `dirty`, `path`: the session;
    /// - `roots`, `plugins`, `audio`, `midi`, `pluginRecovery`: from the library, plug-in, audio and
    ///   MIDI areas (`library.roots`, `plugins.list`, `audio.devices`, `midi.devices`,
    ///   `plugins.recovery`); empty while an area is not implemented;
    /// - `warning`: startup problems to show; `audioError`: the session's audio could not be
    ///   prepared; `openPath`: a project file the system asked to open before the interface was ready.
    fn ready(&self, _: NoPayload) -> Result<Value> {
        let project = self.session.project();
        let mut warnings = self.session.startup_warnings();
        warnings.extend(self.settings.load_warning().map(str::to_string));
        // The window gets the session even when its audio cannot be prepared: it often holds unsaved work.
        let audio_error = project.as_ref().and_then(|project| self.session.prepare_audio(project, true).err());
        let no_devices = || json!({ "inputs": [], "outputs": [] });
        let mut result = json!({
            "version": env!("CARGO_PKG_VERSION"),
            "commands": self.calls.implemented(),
            "project": project.as_ref().map(st_project::Project::to_value),
            "dirty": self.session.dirty(),
            "path": self.session.path(),
            "roots": self.area_part("library.roots", json!([]), &mut warnings),
            "plugins": self.area_part("plugins.list", json!([]), &mut warnings),
            "audio": self.area_part("audio.devices", no_devices(), &mut warnings),
            "midi": self.area_part("midi.devices", no_devices(), &mut warnings),
        });
        if self.calls.implements("plugins.recovery") {
            result["pluginRecovery"] = self.area_part("plugins.recovery", Value::Null, &mut warnings);
        }
        if !warnings.is_empty() {
            result["warning"] = json!(warnings.join(" "));
        }
        if let Some(error) = audio_error {
            result["audioError"] = json!(error.message());
        }
        if let Some(path) = self.session.take_open_request() {
            result["openPath"] = json!(path);
        }
        Ok(result)
    }
}

/// Hands links and files to the desktop.
pub trait Opener: Send + Sync + 'static {
    /// Opens a web address in the default browser.
    fn open_url(&self, url: &str) -> Result<()>;
    /// Shows a file in the file manager.
    fn reveal(&self, path: &Path) -> Result<()>;
}

/// [`Opener`] through the platform's launcher: `open` on macOS, `explorer` on Windows, `xdg-open`
/// elsewhere (which opens the containing folder, as it cannot select a file).
pub struct SystemOpener;

fn launch(program: &str, arguments: &[&std::ffi::OsStr]) -> Result<()> {
    Command::new(program).args(arguments).spawn().map(|_| ()).map_err(|error| Error::new(format!("Could not start {program}: {error}.")))
}

impl Opener for SystemOpener {
    fn open_url(&self, url: &str) -> Result<()> {
        if cfg!(target_os = "macos") {
            launch("open", &[url.as_ref()])
        } else if cfg!(target_os = "windows") {
            launch("explorer", &[url.as_ref()])
        } else {
            launch("xdg-open", &[url.as_ref()])
        }
    }

    fn reveal(&self, path: &Path) -> Result<()> {
        if cfg!(target_os = "macos") {
            launch("open", &["-R".as_ref(), path.as_os_str()])
        } else if cfg!(target_os = "windows") {
            let mut select = std::ffi::OsString::from("/select,");
            select.push(path);
            launch("explorer", &[&select])
        } else {
            launch("xdg-open", &[path.parent().unwrap_or(path).as_os_str()])
        }
    }
}

struct External {
    opener: Arc<dyn Opener>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ExternalPayload {
    url: Option<String>,
    path: Option<PathBuf>,
    reveal: bool,
}

impl External {
    /// `app.external`: `{url}` opens a web link; `{path, reveal: true}` shows a file in the file manager.
    fn open(&self, payload: ExternalPayload) -> Result<Value> {
        if let (Some(path), true) = (&payload.path, payload.reveal) {
            if !path.is_absolute() || !path.exists() {
                return Err(Error::new("This file is unavailable. Reconnect its drive or rescan the library."));
            }
            self.opener.reveal(path)?;
            return Ok(json!({ "opened": true }));
        }
        let url = payload.url.unwrap_or_default();
        let web = ["https://", "http://"].iter().any(|scheme| url.len() > scheme.len() && url[..scheme.len()].eq_ignore_ascii_case(scheme));
        if !web || url.chars().any(|character| character.is_control() || character.is_whitespace()) {
            return Err(Error::new("Only web links can be opened here."));
        }
        self.opener.open_url(&url)?;
        Ok(json!({ "opened": true }))
    }
}

/// The quit handshake.
///
/// The interface owns the document, so the host cannot quit on its own:
///
/// 1. The window is asked to close. The executable calls [`Lifecycle::request_quit`]; with a ready
///    interface this sends the event `app.quitRequested` and returns [`QuitRequest::Asked`].
/// 2. The interface answers with the command `app.quitAnswer`:
///    - `{state: "deciding"}` at once: it is alive and is flushing edits or asking about unsaved work;
///    - `{state: "stay", reason?}`: quitting was cancelled (by the person, or because an operation
///      such as an export must finish first);
///    - `{state: "proceed", project?}`: quit. The host takes the final project, writes the recovery
///      copy and tells the executable. If that fails the answer is an error, and the interface may
///      repeat it with `force: true` after asking the person.
/// 3. The executable hears each step through the function given to [`Lifecycle::on_progress`]. An
///    interface that has not answered at all after a few seconds is hung: the executable offers to
///    quit anyway and calls [`Lifecycle::give_up`] if the person declines.
pub struct Lifecycle {
    events: EventSink,
    session: Arc<Session>,
    state: Mutex<Quit>,
}

#[derive(Default)]
struct Quit {
    phase: Phase,
    listener: Option<Arc<dyn Fn(QuitProgress) + Send + Sync>>,
    /// Told `true` when a quit question opens and `false` when it closes with "stay".
    fence: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Running,
    Asked,
    Deciding,
    Quitting,
}

/// What [`Lifecycle::request_quit`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitRequest {
    /// No interface is ready (still loading, failed to load, or none attached): there is nothing to
    /// ask and nothing to lose; quit now.
    QuitNow,
    /// The interface was asked; the answer arrives through [`Lifecycle::on_progress`].
    Asked,
    /// A question is already open.
    AlreadyAsked,
}

/// A step of the quit handshake, reported to the executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuitProgress {
    /// The interface is alive and working on the answer: stop any "not responding" timer.
    Deciding,
    /// Do not quit. The reason, if any, has already been shown by the interface.
    Stay(Option<String>),
    /// The session is secured: shut down and exit.
    Proceed,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuitAnswer {
    pub state: String,
    pub reason: Option<String>,
    pub project: Option<Value>,
    /// Quit even if the session cannot be secured.
    pub force: bool,
}

impl Lifecycle {
    fn state(&self) -> MutexGuard<'_, Quit> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sets the function that hears the handshake's steps. It is called on a bridge worker thread.
    pub fn on_progress(&self, listener: impl Fn(QuitProgress) + Send + Sync + 'static) {
        self.state().listener = Some(Arc::new(listener));
    }

    /// Sets the function that fences work which must not start while a quit is being decided
    /// (AI control stops admitting requests): it is called with `true` when the interface is
    /// asked, and with `false` when the answer is "stay" or the executable gives up. After
    /// "proceed" it is not called again. It runs on the thread that caused the step and must
    /// not block.
    pub fn set_fence(&self, fence: impl Fn(bool) + Send + Sync + 'static) {
        self.state().fence = Some(Arc::new(fence));
    }

    fn fence(&self, quitting: bool) {
        let fence = self.state().fence.clone();
        if let Some(fence) = fence {
            fence(quitting);
        }
    }

    /// Step 1 of the handshake (see the type's documentation).
    pub fn request_quit(&self) -> QuitRequest {
        let mut state = self.state();
        if state.phase != Phase::Running {
            return QuitRequest::AlreadyAsked;
        }
        if !self.events.has_ready_interface() {
            state.phase = Phase::Quitting;
            return QuitRequest::QuitNow;
        }
        state.phase = Phase::Asked;
        drop(state);
        self.fence(true);
        self.events.emit("app.quitRequested", json!({}));
        QuitRequest::Asked
    }

    /// True while the interface has been asked and has not answered at all.
    pub fn is_unanswered(&self) -> bool {
        self.state().phase == Phase::Asked
    }

    /// The executable stopped waiting for an answer and keeps running.
    pub fn give_up(&self) {
        let mut state = self.state();
        if state.phase != Phase::Quitting && state.phase != Phase::Running {
            state.phase = Phase::Running;
            drop(state);
            self.fence(false);
        }
    }

    fn report(&self, progress: QuitProgress) {
        let listener = self.state().listener.clone();
        if let Some(listener) = listener {
            listener(progress);
        }
    }

    /// `app.quitAnswer` (step 2).
    pub fn answer(&self, answer: QuitAnswer) -> Result<Value> {
        let phase = self.state().phase;
        if phase == Phase::Running {
            return Err(Error::new("SuperTracker is not waiting for an answer about quitting."));
        }
        match answer.state.as_str() {
            "deciding" => {
                let mut state = self.state();
                if state.phase == Phase::Asked {
                    state.phase = Phase::Deciding;
                }
                drop(state);
                self.report(QuitProgress::Deciding);
                Ok(json!({}))
            }
            "stay" => {
                self.give_up();
                self.report(QuitProgress::Stay(answer.reason));
                Ok(json!({}))
            }
            "proceed" => {
                if let Err(error) = self.session.secure_for_quit(answer.project) {
                    if !answer.force {
                        // The question stays open: the interface decides between "keep working" and "quit anyway".
                        self.state().phase = Phase::Deciding;
                        self.report(QuitProgress::Deciding);
                        return Err(Error::new(format!("Your latest changes could not be secured: {error}")));
                    }
                }
                self.state().phase = Phase::Quitting;
                self.session.stop();
                self.report(QuitProgress::Proceed);
                Ok(json!({ "quitting": true }))
            }
            other => Err(Error::new(format!("`app.quitAnswer` does not know the state “{other}”."))),
        }
    }
}

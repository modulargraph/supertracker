//! The service `st-host` registers: the `mk3.*` commands, the stored setting and the events.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::driver::{DriverCallbacks, DriverOptions, DriverStatus, Mk3Driver};
use crate::screen::font::Fonts;
use crate::transport::{HidTransport, NoAgents, ScreenTransport, VendorAgents};

/// An error of an `mk3.*` command; its text is the reply's `error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    /// The message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// The settings section `mk3`. Support for the controller is in development and off unless the
/// user turns it on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Mk3Settings {
    /// Maschine MK3 support is on.
    pub enabled: bool,
}

/// Payload of `mk3.configure`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Configure {
    /// Turn Maschine MK3 support on or off. Required.
    pub enabled: Option<bool>,
}

/// Payload of `mk3.connect`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Connect {
    /// Take over the screens (stops Native Instruments' agents while this application has them).
    pub exclusive: bool,
}

/// Payload of `mk3.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct StatusRequest {
    /// Include report counters, first reports and transfer counters.
    pub diagnostics: bool,
}

/// Result of `mk3.status`, `mk3.connect`, `mk3.disconnect` and `mk3.configure`, and payload of
/// the `mk3.status` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    /// The driver's state.
    #[serde(flatten)]
    pub driver: DriverStatus,
    /// Maschine MK3 support is on in Settings.
    pub enabled: bool,
    /// Counters, when asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Value>,
}

/// Result of `mk3.leds`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LedsResult {
    /// Always true.
    pub ok: bool,
}

/// Result of `mk3.screen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScreenResult {
    /// Whether the screens are taken over (and the display lists therefore shown).
    pub screens: bool,
}

/// Receives the service's events: `(name, payload)`. `st-host` passes `EventSink::emit`.
pub type EventCallback = Box<dyn Fn(&str, Value) + Send + Sync>;
/// Stores the settings section `mk3`. `st-host` passes `|s| settings.set("mk3", s)`.
pub type SaveSettings = Box<dyn Fn(&Mk3Settings) -> Result<(), String> + Send + Sync>;

/// What the service is built from.
pub struct Mk3Options {
    /// The controller's HID interface.
    pub hid: Arc<dyn HidTransport>,
    /// The controller's display interface.
    pub screens: Arc<dyn ScreenTransport>,
    /// The vendor software that must step aside for the screens.
    pub agents: Arc<dyn VendorAgents>,
    /// Fonts for the screens' text.
    pub fonts: Arc<Fonts>,
    /// The stored settings section.
    pub settings: Mk3Settings,
    /// Called when the setting changes.
    pub save_settings: SaveSettings,
    /// Receives `mk3.input` and `mk3.status`.
    pub events: EventCallback,
    /// Receives log lines (take-over steps, failed transfers).
    pub log: Box<dyn Fn(&str) + Send + Sync>,
    /// Render every frame to PNG files in this folder as well, also without the screens: layouts
    /// can be checked without the device. `<dir>/left.png` is the rendered bitmap,
    /// `left-device.png` what the device shows after 16-bit conversion and packet decoding.
    pub dump_dir: Option<PathBuf>,
    /// Keep a numbered copy of every dumped frame.
    pub dump_all: bool,
    /// How often an absent controller is looked for.
    pub poll_interval: Duration,
    /// How long claiming the display interface is retried while its previous owner lets go.
    pub screen_open_timeout: Duration,
    /// The shortest time between two frames on a screen.
    pub frame_interval: Duration,
    /// Do not connect at start even when support is on (`SUPERTRACKER_NO_MK3`).
    pub no_auto_connect: bool,
}

impl Mk3Options {
    /// The real controller: system HID and USB, the system's fonts, and on macOS the control of
    /// Native Instruments' agents with its record in `support_dir`. Reads `SUPERTRACKER_NO_MK3`,
    /// `SUPERTRACKER_MK3_SCREEN_DUMP` (a folder) and `SUPERTRACKER_MK3_SCREEN_DUMP_ALL`.
    pub fn system(support_dir: &Path, settings: Mk3Settings, save_settings: SaveSettings, events: EventCallback) -> Self {
        Mk3Options {
            hid: Arc::new(crate::device::SystemHid::new()),
            screens: Arc::new(crate::device::SystemScreens::new()),
            agents: system_agents(support_dir),
            fonts: Arc::new(Fonts::system()),
            settings,
            save_settings,
            events,
            log: Box::new(|message| eprintln!("Maschine MK3: {message}")),
            dump_dir: std::env::var_os("SUPERTRACKER_MK3_SCREEN_DUMP").map(PathBuf::from),
            dump_all: std::env::var_os("SUPERTRACKER_MK3_SCREEN_DUMP_ALL").is_some(),
            poll_interval: Duration::from_secs(1),
            screen_open_timeout: Duration::from_secs(4),
            frame_interval: Duration::from_micros(33_333),
            no_auto_connect: std::env::var_os("SUPERTRACKER_NO_MK3").is_some(),
        }
    }

    /// A controller made of the given parts (see [`crate::fake`]), with the built-in font, no
    /// vendor agents, quick polling, and settings and events going nowhere: the base for tests.
    pub fn with(hid: Arc<dyn HidTransport>, screens: Arc<dyn ScreenTransport>) -> Self {
        Mk3Options {
            hid,
            screens,
            agents: Arc::new(NoAgents),
            fonts: Arc::new(Fonts::builtin()),
            settings: Mk3Settings::default(),
            save_settings: Box::new(|_| Ok(())),
            events: Box::new(|_, _| {}),
            log: Box::new(|_| {}),
            dump_dir: None,
            dump_all: false,
            poll_interval: Duration::from_millis(5),
            screen_open_timeout: Duration::ZERO,
            frame_interval: Duration::from_millis(2),
            no_auto_connect: false,
        }
    }
}

#[cfg(target_os = "macos")]
fn system_agents(support_dir: &Path) -> Arc<dyn VendorAgents> {
    Arc::new(crate::agents::NiAgents::new(crate::agents::state_path(support_dir)))
}

#[cfg(not(target_os = "macos"))]
fn system_agents(_support_dir: &Path) -> Arc<dyn VendorAgents> {
    Arc::new(NoAgents)
}

struct Shared {
    settings: Mutex<Mk3Settings>,
    save_settings: SaveSettings,
    events: EventCallback,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The Maschine MK3 for the host.
///
/// Events: `mk3.input` (one per control change, see [`crate::protocol::InputEvent::payload`]; with
/// `cancelled: true` for releases made up when the controller went away) and `mk3.status` (the
/// [`Status`], on every change).
///
/// `mk3.connect`, `mk3.disconnect` and `mk3.configure` can take seconds (stopping and starting the
/// vendor's agents): register them as blocking commands.
pub struct Mk3Service {
    driver: Mk3Driver,
    shared: Arc<Shared>,
    no_auto_connect: bool,
}

impl Mk3Service {
    /// Creates the service. No hardware is touched until [`start`](Self::start) or a command.
    pub fn new(options: Mk3Options) -> Arc<Self> {
        let shared = Arc::new(Shared { settings: Mutex::new(options.settings), save_settings: options.save_settings, events: options.events });
        let (for_input, for_status) = (shared.clone(), shared.clone());
        let driver = Mk3Driver::new(DriverOptions {
            hid: options.hid,
            screens: options.screens,
            agents: options.agents,
            fonts: options.fonts,
            callbacks: DriverCallbacks {
                input: Box::new(move |payload| (for_input.events)("mk3.input", Value::Object(payload))),
                status: Box::new(move |status| {
                    let enabled = lock(&for_status.settings).enabled;
                    (for_status.events)("mk3.status", json!(Status { driver: status, enabled, diagnostics: None }));
                }),
                log: options.log,
                raw_report: Box::new(|_| {}),
            },
            dump_dir: options.dump_dir,
            dump_all: options.dump_all,
            poll_interval: options.poll_interval,
            screen_open_timeout: options.screen_open_timeout,
            frame_interval: options.frame_interval,
        });
        Arc::new(Mk3Service { driver, shared, no_auto_connect: options.no_auto_connect })
    }

    /// Call once at launch, from a thread that may wait: restores agents a crashed earlier run
    /// left stopped (also when support is off), then, when support is on, starts looking for the
    /// controller in shared mode. Emits `mk3.status`.
    pub fn start(&self) {
        self.driver.recover_agents();
        if self.enabled() && !self.no_auto_connect {
            self.driver.connect(false);
        } else {
            (self.shared.events)("mk3.status", json!(self.status_now(false)));
        }
    }

    /// The application quits: LEDs off, screens blank and released, agents restored. Returns when
    /// that is done.
    pub fn shutdown(&self) {
        self.driver.shutdown();
    }

    /// The driver underneath (the probe uses it directly).
    pub fn driver(&self) -> &Mk3Driver {
        &self.driver
    }

    fn enabled(&self) -> bool {
        lock(&self.shared.settings).enabled
    }

    fn with_enabled(&self, driver: DriverStatus) -> Status {
        Status { driver, enabled: self.enabled(), diagnostics: None }
    }

    fn status_now(&self, diagnostics: bool) -> Status {
        Status { diagnostics: diagnostics.then(|| self.driver.diagnostics()), ..self.with_enabled(self.driver.status()) }
    }

    /// Command `mk3.configure`: `{enabled: bool}` turns support on (and connects in shared mode)
    /// or off (and releases the controller).
    pub fn configure(&self, request: Configure) -> Result<Status, Error> {
        let enabled = request.enabled.ok_or_else(|| Error("Choose whether Maschine MK3 support is on.".into()))?;
        let settings = Mk3Settings { enabled };
        (self.shared.save_settings)(&settings).map_err(|error| Error(format!("The Maschine MK3 setting could not be saved: {error}")))?;
        *lock(&self.shared.settings) = settings;
        Ok(self.with_enabled(if enabled { self.driver.connect(false) } else { self.driver.disconnect() }))
    }

    /// Command `mk3.connect`: `{exclusive?: bool}`. Refused while support is off. The result tells
    /// whether the screens were opened (`exclusive`, `screens`) or why not (`error`).
    pub fn connect(&self, request: Connect) -> Result<Status, Error> {
        if !self.enabled() {
            return Err(Error("Maschine MK3 support is in development and off. Turn it on in Settings first.".into()));
        }
        Ok(self.with_enabled(self.driver.connect(request.exclusive)))
    }

    /// Command `mk3.disconnect`: releases the controller to other software.
    pub fn disconnect(&self, _: serde::de::IgnoredAny) -> Result<Status, Error> {
        Ok(self.with_enabled(self.driver.disconnect()))
    }

    /// Command `mk3.status`: `{diagnostics?: bool}`.
    pub fn status(&self, request: StatusRequest) -> Result<Status, Error> {
        Ok(self.status_now(request.diagnostics))
    }

    /// Command `mk3.leds`: a partial LED update (see [`crate::protocol::LedState::apply`]).
    pub fn leds(&self, payload: Value) -> Result<LedsResult, Error> {
        self.driver.set_leds(&payload);
        Ok(LedsResult { ok: true })
    }

    /// Command `mk3.screen`: `{left?: [...], right?: [...]}` display lists (see
    /// [`crate::screen::ops::DrawOp::parse`]). Returns at once; frames are drawn on the driver's
    /// thread.
    pub fn screen(&self, payload: Value) -> Result<ScreenResult, Error> {
        Ok(ScreenResult { screens: self.driver.set_screens(&payload) })
    }
}

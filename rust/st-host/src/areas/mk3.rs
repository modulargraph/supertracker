//! The Maschine MK3 area: the controller driver (`st-mk3`).
//!
//! [`install`] creates the [`Mk3Service`], registers `mk3.configure`, `mk3.connect`,
//! `mk3.disconnect` (blocking: stopping and restoring Native Instruments' agents takes
//! seconds), `mk3.status`, `mk3.leds` and `mk3.screen` (quick), and starts the service on a
//! thread of its own: it restores agents a crashed earlier run left stopped and, only when
//! support is on in the settings (section `mk3`, off by default), looks for the controller.
//!
//! Events: `mk3.input` (one per control change) and `mk3.status`.
//!
//! # Leaving the controller in a sane state
//!
//! [`Mk3Service::shutdown`] switches the LEDs off, blanks and releases the screens and starts
//! the vendor's agents again. The host calls it when it quits
//! ([`crate::areas::services::Services::shutdown`]); the executable must also call that on
//! termination signals (`st-app` does, for SIGINT, SIGTERM and SIGHUP).
//!
//! [`probe`] is the `--mk3-probe` diagnostic of the executable.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use st_mk3::transport::{HidTransport, ScreenTransport, VendorAgents};
use st_mk3::{DriverOptions, Mk3Service};

use crate::commands::Commands;
use crate::{EventSink, Settings};

/// Which controller the host drives.
#[derive(Clone)]
pub enum Mk3Devices {
    /// The real controller: system HID and USB.
    System,
    /// A controller made of these parts (`st_mk3::fake`, for tests).
    Parts { hid: Arc<dyn HidTransport>, screens: Arc<dyn ScreenTransport> },
}

/// How [`crate::install`] sets the controller up.
#[derive(Clone)]
pub struct Mk3Options {
    pub devices: Mk3Devices,
}

const SETTINGS_SECTION: &str = "mk3";

/// Creates the controller service, registers the `mk3.*` commands and starts the service.
pub fn install(commands: &mut Commands, support: &Path, settings: &Arc<Settings>, events: EventSink, options: &Mk3Options) -> Arc<Mk3Service> {
    let stored: st_mk3::Mk3Settings = settings.get(SETTINGS_SECTION);
    let save: st_mk3::SaveSettings = Box::new({
        let settings = settings.clone();
        move |value| settings.set(SETTINGS_SECTION, value).map_err(|error| error.to_string())
    });
    let emit: st_mk3::EventCallback = Box::new(move |name, payload| events.emit(name, payload));
    let service = Mk3Service::new(match &options.devices {
        Mk3Devices::System => st_mk3::Mk3Options::system(support, stored, save, emit),
        Mk3Devices::Parts { hid, screens } => st_mk3::Mk3Options { settings: stored, save_settings: save, events: emit, ..st_mk3::Mk3Options::with(hid.clone(), screens.clone()) },
    });
    commands
        .with(&service)
        .register_blocking("mk3.configure", Mk3Service::configure)
        .register_blocking("mk3.connect", Mk3Service::connect)
        .register_blocking("mk3.disconnect", Mk3Service::disconnect)
        .register("mk3.status", Mk3Service::status)
        .register("mk3.leds", Mk3Service::leds)
        .register("mk3.screen", Mk3Service::screen);
    let starting = service.clone();
    // `start` may wait (the vendor's agents); a host that cannot start the thread simply has
    // no controller until `mk3.connect`.
    let _ = std::thread::Builder::new().name("mk3-start".to_string()).spawn(move || starting.start());
    service
}

#[cfg(target_os = "macos")]
fn system_agents(support: &Path) -> Arc<dyn VendorAgents> {
    Arc::new(st_mk3::agents::NiAgents::new(st_mk3::agents::state_path(support)))
}

#[cfg(not(target_os = "macos"))]
fn system_agents(_support: &Path) -> Arc<dyn VendorAgents> {
    Arc::new(st_mk3::transport::NoAgents)
}

/// `supertracker --mk3-probe <seconds> [--exclusive] [--raw]`: the hardware diagnostic of
/// `st_mk3::probe`, on the real controller, printing to standard output. Returns the exit code
/// (0, or 2 when no controller was found). It always releases the device and restores the
/// vendor's agents (their record is kept in `support`).
pub fn probe(support: &Path, seconds: f64, exclusive: bool, raw: bool) -> i32 {
    let options = DriverOptions {
        hid: Arc::new(st_mk3::device::SystemHid::new()),
        screens: Arc::new(st_mk3::device::SystemScreens::new()),
        agents: system_agents(support),
        fonts: Arc::new(st_mk3::screen::font::Fonts::system()),
        callbacks: st_mk3::DriverCallbacks::default(),
        dump_dir: std::env::var_os("SUPERTRACKER_MK3_SCREEN_DUMP").map(std::path::PathBuf::from),
        dump_all: std::env::var_os("SUPERTRACKER_MK3_SCREEN_DUMP_ALL").is_some(),
        poll_interval: Duration::from_secs(1),
        screen_open_timeout: Duration::from_secs(4),
        frame_interval: Duration::from_micros(33_333),
    };
    st_mk3::probe::run(options, st_mk3::probe::ProbeOptions { seconds, exclusive, raw }, std::io::stdout())
}

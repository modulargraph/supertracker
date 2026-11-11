//! Driver for the Native Instruments Maschine MK3 controller: pads with pressure, buttons,
//! encoders and touch strip in; LEDs and the two 480 × 272 displays out. See
//! `docs/hardware/MaschineMK3-*.md` for the device and `docs/ARCHITECTURE.md` for the crate's
//! place in the host.
//!
//! The interface drives the controller through the `mk3.*` commands and receives `mk3.input` and
//! `mk3.status` events (`ui/mk3-api.js`); [`Mk3Service`] is that surface.
//!
//! | Module | Contents | Hardware? |
//! |---|---|---|
//! | [`protocol`] | input reports to events, LED state to output reports | no |
//! | [`screen`] | display lists, the rasterizer, fonts, pixel formats and bulk packets | no |
//! | [`transport`] | the three interfaces to hardware and vendor software | (interfaces) |
//! | [`fake`] | an in-memory controller for tests | no |
//! | [`device`] | the real HID and USB transports (`async-hid`, `nusb`) | yes |
//! | [`agents`] | Native Instruments' background agents on macOS | yes (macOS) |
//! | [`Mk3Driver`] | ownership: shared mode, the exclusive take-over of the screens, release, shutdown | through the interfaces |
//! | [`probe`] | the `--mk3-probe` diagnostic | through the interfaces |
//!
//! # Registering in `st-host`
//!
//! ```ignore
//! let mk3 = st_mk3::Mk3Service::new(st_mk3::Mk3Options::system(
//!     &support_dir,
//!     settings.get("mk3"),
//!     Box::new({ let settings = settings.clone(); move |value| settings.set("mk3", value).map_err(|e| e.to_string()) }),
//!     Box::new({ let events = bridge.events(); move |name, payload| events.emit(name, payload) }),
//! ));
//! bridge.commands().with(&mk3)
//!     .register_blocking("mk3.configure", st_mk3::Mk3Service::configure)
//!     .register_blocking("mk3.connect", st_mk3::Mk3Service::connect)
//!     .register_blocking("mk3.disconnect", st_mk3::Mk3Service::disconnect)
//!     .register("mk3.status", st_mk3::Mk3Service::status)
//!     .register("mk3.leds", st_mk3::Mk3Service::leds)
//!     .register("mk3.screen", st_mk3::Mk3Service::screen);
//! std::thread::spawn({ let mk3 = mk3.clone(); move || mk3.start() });   // at launch
//! // when quitting, and on SIGINT/SIGTERM/SIGHUP:  mk3.shutdown();
//! ```
//!
//! # Leaving the device in a sane state
//!
//! Release ([`Mk3Driver::disconnect`]) and shutdown ([`Mk3Driver::shutdown`]) switch every LED
//! off, blank both screens, release the display interface and start the vendor's agents again. A
//! take-over that fails half way does the same. A crash while the agents are stopped is repaired
//! at the next launch from a record written before they were stopped. The library installs no
//! signal handlers: the executable must call [`Mk3Service::shutdown`] on termination signals.
//!
//! # What is verified and what is not
//!
//! Tested here, without hardware: the protocol, rendering and
//! packet encoding (with the built-in font), and the whole driver against [`fake`] transports
//! (ownership changes in every order, failures, hot-plug, LED and frame coalescing).
//!
//! **Not verified, because no controller was available:** everything in [`device`] and the
//! process control in [`agents`]. In particular:
//!
//! - that `async-hid` delivers the controller's input reports with their report id as the first
//!   byte on each system (a missing id is restored from the report's size), and that its shared
//!   open on macOS coexists with the vendor's agents;
//! - that output reports written through `async-hid` light the LEDs (on macOS it uses
//!   `IOHIDDeviceSetReport`; on Linux it writes to hidraw);
//! - that `nusb` can claim interface 5 once the agents are stopped. `nusb` has no seize, so an
//!   interface that stays held makes the attempt fail with a message;
//! - the look of text: glyphs come from the system's font files through `ab_glyph`, not from
//!   CoreText, so metrics differ slightly from text the system draws. Set
//!   `SUPERTRACKER_MK3_SCREEN_DUMP=<folder>` to see every frame as PNG without the device;
//! - the controller's state at connection: `async-hid` cannot ask for it (a GET_REPORT of the
//!   button report), so knob baselines and held buttons are learned from the first report the
//!   controller sends by itself.
//!
//! The macOS code type-checks from Linux (`cargo check -p st-mk3 --target aarch64-apple-darwin`).

#![warn(missing_docs)]

pub mod agents;
pub mod device;
mod driver;
pub mod fake;
mod hid;
pub mod probe;
pub mod protocol;
pub mod screen;
mod screens;
mod service;
pub mod transport;

pub use driver::{DriverCallbacks, DriverOptions, DriverStatus, Mk3Driver, RawReportCallback};
pub use service::{Configure, Connect, Error, EventCallback, LedsResult, Mk3Options, Mk3Service, Mk3Settings, SaveSettings, ScreenResult, Status, StatusRequest};

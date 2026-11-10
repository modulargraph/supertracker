//! SuperTracker's application core: the bridge between the interface (a web page) and the host,
//! the session it holds for the page, and settings. No window or web view code lives here; the
//! executable (`st-app`) supplies transports and file choosers. See `docs/RUST_ARCHITECTURE.md`.
//!
//! # The bridge
//!
//! The interface sends requests, `{"id": 7, "command": "project.save", "payload": {...}}`, and gets
//! exactly one reply for each, `{"id": 7, "ok": true, "result": ...}` or
//! `{"id": 7, "ok": false, "error": "..."}`. The host sends events, `{"type": "status", "payload":
//! {...}}`, whenever it has something to say ([`protocol`]).
//!
//! Everything the interface can ask for is one registration in the command table ([`Commands`]):
//! a name, a payload type that `serde` reads, and a handler that returns a `Result` whose value
//! `serde` writes and whose error is a message for the user.
//!
//! ```
//! use std::sync::Arc;
//! use serde::{Deserialize, Serialize};
//! use st_host::{Bridge, NoPayload, Result};
//!
//! // A service of some area. Services are shared between threads: `Send + Sync`.
//! struct Tuner {
//!     events: st_host::EventSink,
//! }
//!
//! #[derive(Deserialize)]
//! #[serde(default)]                       // missing fields take their defaults,
//! struct Tune {                           // unknown fields are ignored
//!     note: u8,
//!     cents: f64,
//! }
//! impl Default for Tune {
//!     fn default() -> Self { Tune { note: 69, cents: 0.0 } }
//! }
//!
//! #[derive(Serialize)]
//! struct Tuned {
//!     hertz: f64,
//! }
//!
//! impl Tuner {
//!     fn tune(&self, tune: Tune) -> Result<Tuned> {
//!         if tune.note > 127 {
//!             st_host::fail!("Choose a note from 0 to 127.");
//!         }
//!         let hertz = 440.0 * 2f64.powf((f64::from(tune.note) - 69.0 + tune.cents / 100.0) / 12.0);
//!         self.events.emit("tuner.changed", Tuned { hertz });   // an event, from any thread
//!         Ok(Tuned { hertz })
//!     }
//!     fn reference(&self, _: NoPayload) -> Result<f64> {
//!         Ok(440.0)
//!     }
//! }
//!
//! let mut bridge = Bridge::builder();
//! let tuner = Arc::new(Tuner { events: bridge.events() });
//! bridge.commands().with(&tuner)
//!     .register("tuner.tune", Tuner::tune)                  // one line per command
//!     .register("tuner.reference", Tuner::reference);
//! let bridge = bridge.start();
//!
//! // A transport attaches the interface and feeds it the texts it receives:
//! let outbox = st_host::Outbox::new();
//! let page = bridge.connect(outbox.clone());
//! page.receive(r#"{"id": 1, "command": "ready"}"#);
//! page.receive(r#"{"id": 2, "command": "tuner.tune", "payload": {"note": 57}}"#);
//! # let mut texts = Vec::new();
//! # while texts.len() < 3 {
//! #     if let st_host::Next::Message(text) = outbox.wait(std::time::Duration::from_secs(5)) { texts.push(text) } else { break }
//! # }
//! # assert_eq!(texts[2], r#"{"id":2,"ok":true,"result":{"hertz":220.0}}"#);
//! # assert_eq!(texts[1], r#"{"payload":{"hertz":220.0},"type":"tuner.changed"}"#);
//! # assert!(texts[0].contains("`ready` is not a command"));
//! ```
//!
//! # Where handlers run
//!
//! Handlers are plain synchronous functions; the registration decides the thread.
//!
//! - [`Commands::register`]: **quick commands** run on the one bridge thread, in the order the
//!   interface sent them. This is the default because order matters for most of them (`note.on`
//!   before its `note.off`, `transport.seek` before `transport.play`). They must not wait for
//!   anything slow; one that takes longer than a quarter of a second is reported in the log.
//! - [`Commands::register_blocking`]: **commands that may take long** (file dialogs, saves,
//!   exports, scans) each run on a worker thread, concurrently with everything else. The interface
//!   keeps working, and other requests are answered, while a Save window is open.
//! - [`Commands::register_deferred`]: the handler gets the typed [`Reply`] and answers later from
//!   any thread (the window's event loop for a native menu, an export's own thread).
//!
//! There is no async runtime: handlers that wait simply block their worker thread, which keeps
//! them as easy to write and test as any other function, and the host has only a few such requests
//! in flight at a time. Services guard their own state (a mutex held briefly, never across a wait).
//!
//! Every request gets exactly one reply: a handler's return value, or its error; an error naming the
//! command when no handler is registered ([`catalog::missing_message`]), when the payload does not
//! fit, when a [`Reply`] is dropped unanswered, or when the host shuts down first
//! ([`Bridge::shutdown`]).
//!
//! # What is here
//!
//! | Module | Contents |
//! |---|---|
//! | [`protocol`] | the wire format |
//! | [`commands`], [`bridge`] | the command table and the running bridge: [`Commands`], [`Bridge`], [`Connection`], [`EventSink`], [`Calls`] |
//! | [`transport`] | [`Transport`], the trait a transport implements, and [`Outbox`], the queue both transports are built on |
//! | [`catalog`] | every command of the interface with its owner crate; the source of `docs/RUST_PORT_STATUS.md` |
//! | [`session`], [`document`] | the open project and `engine.project`, `project.*`, `dialog.*` |
//! | [`settings`] | the settings file |
//! | [`dialogs`] | the file chooser trait, and scripted answers for tests |
//! | [`app`] | `ready`, `app.external`, the quit handshake, and [`install`] |
//! | [`testing`] | [`testing::TestHost`] for testing commands as the interface would call them |

pub mod app;
pub mod bridge;
pub mod catalog;
pub mod commands;
pub mod dialogs;
pub mod document;
mod error;
mod paths;
pub mod protocol;
pub mod session;
pub mod settings;
pub mod testing;
pub mod transport;

pub use app::{install, Host, HostOptions, Lifecycle, Opener, QuitProgress, QuitRequest, SystemOpener};
pub use bridge::{Bridge, BridgeBuilder, CallError, Calls, Connection, EventSink};
pub use commands::{Commands, Lane, NoPayload, Reply, Scoped};
pub use dialogs::{DialogKind, Dialogs, FileDialog, FileFilter, NoDialogs, ScriptedDialogs};
pub use error::{Error, Result};
pub use paths::support_dir;
pub use session::{ProjectAudio, PublishOptions, Session};
pub use settings::Settings;
pub use transport::{Next, Outbox, Outgoing, Transport};

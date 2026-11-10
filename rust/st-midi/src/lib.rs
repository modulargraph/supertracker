//! MIDI for SuperTracker's host: devices with hot-plug, input routing, sample-accurate output to
//! external instruments, and MIDI clock. See `docs/RUST_ARCHITECTURE.md`.
//!
//! # Layers
//!
//! | Module | Contents | Devices? |
//! |---|---|---|
//! | [`message`] | messages and the byte-stream parser (running status, real-time bytes, system-exclusive skipping) | no |
//! | [`held`] | what each input holds down, for the releases an unplugged device owes | no |
//! | [`sustain`] | which note releases each sustain pedal holds back | no |
//! | [`router`] | where input goes: track and instrument, chord map, transport control | no |
//! | [`timebase`] | the render clock: graph frames and device time | no |
//! | [`clock`] | MIDI beat clock and Start/Stop/Continue from the transport position; the stored clock settings | no |
//! | [`schedule`] | timestamps of scheduled notes, the lock-free queue from the audio thread, the sender's backlog | no |
//! | [`ports`], [`memory`], [`midir_backend`] | the device layer: an interface, in-memory ports for tests, real devices through `midir` | yes |
//! | [`host`] | [`MidiHost`]: enumeration, hot-plug, input connections, output routes | yes |
//! | [`output`] | the sender thread and [`RenderOutput`], the audio thread's handle | yes |
//! | [`MidiService`] | the `midi.*` commands, events, and the calls of the audio side | yes |
//!
//! # Output timing
//!
//! The audio callback hands over each event with its frame offset in the render block
//! ([`RenderOutput::note`], [`RenderOutput::clock`]); the event's device time is the block's device
//! time plus the offset ([`schedule::note_timestamp`]). Nothing on that path allocates, locks or
//! calls the system: messages cross to the sender thread through a bounded lock-free queue, and
//! routes and clock outputs reach the audio thread as atomically published words.
//!
//! `midir` cannot send with a timestamp, so the sender thread ([`output`]) keeps the messages in
//! time order and sends each at its time. See [`midir_backend`] for everything `midir` lacks and
//! what stands in for it.
//!
//! # Registering in `st-host`
//!
//! ```ignore
//! let midi = st_midi::MidiService::new(st_midi::MidiOptions::system(
//!     settings.get("midi.clock"),
//!     Box::new({ let settings = settings.clone(); move |value| settings.set("midi.clock", value).map_err(|e| e.to_string()) }),
//!     Box::new({ let events = bridge.events(); move |name, payload| events.emit(name, payload) }),
//! ))?;
//! bridge.commands().with(&midi)
//!     .register("midi.devices", st_midi::MidiService::devices)
//!     .register("midi.configure", st_midi::MidiService::configure)
//!     .register("midi.clock", st_midi::MidiService::clock_command)
//!     .register("midi.performance", st_midi::MidiService::performance);
//! midi.set_target(audio.clone());              // st-audio's side implements st_midi::InputTarget
//! // on every project publication:  midi.set_project(&project);
//! // when the audio stream starts:  let mut out = midi.render_output();  (moved into the callback)
//! ```
//!
//! # Platforms
//!
//! Linux needs the ALSA development files to build (`libasound2-dev`) and the ALSA sequencer to
//! find devices; without the sequencer the service starts without devices and `midi.devices`
//! carries the reason in `error`. The macOS code (CoreMIDI through `midir`) is type-checked from
//! Linux and has not run on a Mac yet. Tests use [`memory::MemoryPorts`]; no test opens a real
//! device.

#![warn(missing_docs)]

pub mod clock;
pub mod held;
pub mod host;
pub mod memory;
pub mod message;
pub mod midir_backend;
pub mod output;
pub mod ports;
pub mod router;
pub mod schedule;
mod service;
pub mod sustain;
pub mod timebase;

pub use clock::{ClockDevice, ClockRequest, ClockSettings};
pub use host::{ConfigureRequest, Devices, HostEvent, HostOptions, MidiError, MidiHost, OutputRoute};
pub use message::Message;
pub use output::{MidiClock, OutputStats, RenderOutput, Route};
pub use router::{InputAction, InputRouter, InputRouting, InstrumentRoute, RoutedNote, TrackRoute};
pub use service::{
    ClockOutput, ClockStatus, EventCallback, InputTarget, MidiOptions, MidiService, NoteHandling, PerformanceRequest, PerformanceStatus,
    ReturnCompensation, SaveClockSettings, TransportPosition,
};
pub use sustain::{Destination, SustainState};
pub use timebase::{RenderStamp, Timebase};

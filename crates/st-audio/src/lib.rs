//! SuperTracker's audio core: the mixer graph around the engine, audio devices, and one render
//! path for live playback and offline rendering. See `docs/ARCHITECTURE.md`.
//!
//! ```text
//!  control threads                                   audio thread (device callback / offline loop)
//!  ---------------                                   ---------------------------------------------
//!  project (st-project)                              Renderer::render(frames)
//!     |  ProjectSync::apply  -> engine update  ~~~>    engine.render_block   (sequencer, voices)
//!     |  GraphSpec::from_project                        engine events -> plan (automation)
//!     |  Compiler::compile   -> Plan           ~~~>    Plan::process         (strips, inserts,
//!  AudioEngine (transport, notes, devices)                                    sends, buses, master)
//! ```
//!
//! | Module | What it does |
//! |---|---|
//! | [`engine`] | [`AudioEngine`]: the object the host talks to (publish, transport, notes, status, devices, offline render) |
//! | [`project_sync`] | applies a project to the engine, incrementally (pattern reuse, samples installed once) |
//! | [`samples`] | [`SampleSource`]: where decoded PCM comes from (the seam to `st-samples`) |
//! | [`samples_async`] | [`DecodedSamples`]: the source the host runs on, decoding files on worker threads |
//! | [`live`], [`preview`], [`inputs`], [`recording`] | what runs on the live output after the mixer: browser preview voices, the monitored input, the tap of a master take; the input device and what listens to it (meter, monitoring, takes, the line inputs of tracks); takes written by a writer thread |
//! | [`offline`], [`export`], [`render_selection`], [`freeze`] | rendering on a private engine while the live one plays: song / pattern / stems / track export, selections to samples, Track Freeze (printing, and playing the prints) |
//! | [`freeze_recording`] | Track Freeze for sound made outside the computer: the prints recorded from the live engine through a tap on the track's strip |
//! | [`kit_preview`] | the audio preview a kit is saved with: its group bus in the selected pattern, rendered on a private engine |
//! | [`graph`] | the mixer: spec, compiler and render plan, delay compensation, automation, inserts, send effects, meters, the renderer |
//! | [`device`] | outputs: `cpal` devices and the null device |
//! | [`midi`] | the MIDI seam: what the render thread tells MIDI output (external instruments, clock) |
//! | [`plugins`] | plug-in hosting: the plug-ins of a project as instances in container processes ([`plugins::PluginManager`]), as processors of the graph ([`plugins::PluginInsert`]), the catalogue of installed plug-ins and the recovery store |
//!
//! # Real-time rules
//!
//! Everything [`graph::render::Renderer::render`] calls is free of allocation, locks and system
//! calls (it reads the clock for the load meter). State reaches it as immutable plans published
//! through an atomic pointer and as values through single-producer/single-consumer rings; what
//! it replaces goes back to the control thread to be freed. The hand-overs are listed in
//! [`graph::render`]; `tests/realtime.rs` counts allocations on the render path.
//!
//! # Seams for what is not here yet
//!
//! | Follow-up | Seam |
//! |---|---|
//! | Other insert processors | [`graph::insert::InsertFactory`] / [`graph::insert::InsertProcessor`]; an insert the factory cannot make passes through and is reported unavailable. Plug-ins are made by [`plugins::PluginFactory`] once a manager is attached ([`AudioEngine::set_plugins`]) |
//! | Other consumers of note events | [`graph::render::ExternalEvents`] receives the engine's note events on the render thread (a track's plug-in instrument gets its notes inside the plan, through [`graph::insert::InsertProcessor::midi`]) |
//! | MIDI outputs, MIDI clock | [`midi`]: [`AudioEngine::set_midi_output`] puts a [`midi::MidiRender`] on the render thread (the host implements it over `st-midi`) |
//!
//! # Line inputs
//!
//! A track's `audioInput` in the project routes channels of the input device into the track's
//! strip, ahead of its inserts. The project is the only description: every publication tells
//! [`inputs::Inputs`] the enabled routes, the live ones become sources of the mixer
//! ([`graph::spec::LineInputSpec`]), and their audio crosses from the input thread to the
//! render thread through one ring per route ([`inputs::LineFeed`], which also says what happens
//! when the input's clock and the output's disagree). `engine/lines.rs` has the latency a
//! hardware return is given in the delay compensation.
//!
//! # Platforms
//!
//! Tested on Linux (ALSA through `cpal`; building needs `libasound2-dev`). The macOS (CoreAudio)
//! device code is `cpal`'s and has only been type-checked for `aarch64-apple-darwin`. No input
//! device has been opened anywhere: inputs, line inputs and Freeze by recording are tested on
//! the null devices.

// The status object is one large `json!` literal.
#![recursion_limit = "256"]

pub mod device;
pub mod engine;
pub mod export;
pub mod freeze;
pub mod freeze_recording;
pub mod graph;
pub mod inputs;
pub mod kit_preview;
pub mod live;
pub mod midi;
pub mod offline;
pub mod plugins;
pub mod preview;
pub mod project_sync;
pub mod recording;
pub mod render_selection;
pub mod samples;
pub mod samples_async;

pub use engine::{AudioEngine, DeviceChoice, DeviceRequest, DevicesStatus, EngineOptions, InputEdge, PlayOptions};
pub use project_sync::{ApplyReport, SampleProblem};
pub use samples::{MemorySamples, NoSamples, SampleLookup, SamplePcm, SampleSource};
pub use samples_async::{DecodedSamples, LoadEvent};

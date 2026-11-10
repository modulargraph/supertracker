//! SuperTracker's audio core: the mixer graph around the engine, audio devices, and one render
//! path for live playback and offline rendering. See `docs/RUST_ARCHITECTURE.md`.
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
//! | [`graph`] | the mixer: spec, compiler and render plan, delay compensation, automation, inserts, send effects, meters, the renderer |
//! | [`device`] | outputs: `cpal` devices and the null device |
//! | [`midi`] | the MIDI seam: what the render thread tells MIDI output (external instruments, clock) |
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
//! | Sample decoding (`st-samples`) | [`SampleSource`]; [`AudioEngine::samples_changed`] when a decode finishes |
//! | Plug-in instances (container client) | [`graph::insert::InsertFactory`] / [`graph::insert::InsertProcessor`]; an insert the factory cannot make passes through and is reported unavailable |
//! | Plug-in instruments, MIDI outputs | [`graph::render::ExternalEvents`] receives the engine's note events on the render thread |
//! | Export, bounce, freeze, render selection | [`AudioEngine::render_offline`] on an engine made with [`DeviceChoice::None`]; `Plan::latency_frames` is the pre-roll to trim |
//! | Recording, line inputs | the device layer has outputs only; input enumeration is in [`device::system::list`] |
//!
//! # Platforms
//!
//! Tested on Linux (ALSA through `cpal`; building needs `libasound2-dev`). The macOS (CoreAudio)
//! device code is `cpal`'s and has only been type-checked for `aarch64-apple-darwin`.

// The status object is one large `json!` literal.
#![recursion_limit = "256"]

pub mod device;
pub mod engine;
pub mod graph;
pub mod midi;
pub mod project_sync;
pub mod samples;
pub mod samples_async;

pub use engine::{AudioEngine, DeviceChoice, DeviceRequest, DevicesStatus, EngineOptions, InputEdge, PlayOptions};
pub use project_sync::{ApplyReport, SampleProblem};
pub use samples::{MemorySamples, NoSamples, SampleLookup, SamplePcm, SampleSource};
pub use samples_async::{DecodedSamples, LoadEvent};

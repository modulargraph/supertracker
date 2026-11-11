//! Plug-in hosting: the plug-ins of a project as instances in container processes, rendered
//! by the mixer graph. See `docs/PLUGIN_CONTAINER.md` for the container and its client.
//!
//! | Module | What it holds |
//! |---|---|
//! | [`manager`] | [`PluginManager`]: the supervisor, one [`Hosted`] instance per plug-in of the project, their life cycle, losses and restores, events from the containers |
//! | [`insert`] | [`PluginInsert`]: an instance as an [`InsertProcessor`](crate::graph::insert::InsertProcessor) (effect or instrument); [`PluginFactory`] |
//! | [`catalog`] | [`Catalog`]: the installed plug-ins, scanned by container processes, with the cache and the blocklist |
//! | [`recovery`] | [`RecoveryStore`]: plug-ins that crash while loading are held back; lost ones are listed for review |
//! | [`ident`] | [`Identity`]: what a descriptor's `component` names |
//! | [`testing`] | the container executable and the CLAP / VST3 test plug-ins of `crates/plugin-host`, for tests |
//!
//! # How it is wired
//!
//! ```text
//!  AudioEngine::publish(project)
//!     PluginManager::reconcile ............ creates, keeps, re-states and buries instances (control thread)
//!     Compiler::compile ................... PluginFactory::create(insert) -> PluginInsert(Arc<Hosted>)
//!  Renderer::render (audio thread)
//!     Plan::set_block(transport, cycle end)
//!     Plan::engine_event .................. notes of a plug-in instrument's track, and the controllers and
//!                                           pressure played live on it, in one order -> PluginInsert::midi
//!     Plan::playback_begins ............... play, seek: automated parameters back to their base values
//!     Plan::process ....................... automation and step locks -> PluginInsert::parameter_at(frame)
//!                                           the end of a locked row -> PluginInsert::base_parameter_at(frame)
//!                                           PluginInsert::process -> Instance::process_raw (one round trip)
//!  PluginManager's thread ................. container events: lost, latency, restart, editor edits
//!     PluginEvent::Graph -> AudioEngine ... forgets the insert's processor and compiles again (delay compensation)
//! ```
//!
//! # Base values
//!
//! Every parameter of every instance has a base value: the value the user left it at (the
//! loaded state, a value set through [`Hosted::set_parameter`], an edit in the plug-in's
//! editor). [`manager::ParamTable`] holds it where the audio thread can read it. Lanes and step
//! locks move a parameter away from its base value for a while; the processor remembers
//! which ones they moved and returns exactly those, to the base value as it is at that
//! moment: when a locked row ends without a lane to take over, when the project changes,
//! and when playback starts or seeks. A stop returns nothing, so that tails ring out with
//! the values the song left; instead, a state captured while the song does not play has the
//! audio thread return them first ([`Hosted::return_to_base`]), and an offline copy made
//! while it plays starts from the base values. A value read from the plug-in while
//! automation holds the parameter is never taken for a base value. The project needs no
//! copy of the base values: they are in the plug-in's state.
//!
//! # Controllers and pressure
//!
//! [`crate::AudioEngine::expression`] hands a plug-in instrument the channel messages played
//! live on its track (control changes, the pedals among them, channel and poly pressure,
//! pitch bend, program changes). They travel the way live notes do: through the engine's
//! command queue and out again as events of the next block, so notes and messages reach the
//! plug-in in the order they were played, at the block's first frame. What a message becomes
//! for the plug-in is its format's business (the container's backends: MIDI or note
//! expressions for CLAP, mapped parameters and poly pressure events for VST3, MIDI for Audio
//! Units).
//!
//! # Blocks a plug-in misses
//!
//! A plug-in that does not answer a block in time (or whose container is busy with a control
//! operation) costs that block's audio, not its events: the client keeps the notes,
//! controllers and parameter changes and delivers them with the next block the plug-in takes,
//! at its first frame and in order (`stpc_client::Instance::process`). Only when a plug-in
//! stays away for two seconds, or more events wait than a block can carry
//! ([`manager::EVENT_CAPACITY`]), is everything that waits given up; the processor then
//! starts the next block with the pedals up, "all notes off" and a note-off for every note
//! the plug-in may hold, so nothing sticks, and returns to base values are sent again.
//!
//! [`crate::AudioEngine::set_plugins`] attaches a manager to an engine. An engine that renders
//! offline next to a live one (export, freeze) takes [`PluginManager::offline`] of the live
//! manager: separate instances without deadlines that start from the live instances' states.
//!
//! # Platforms
//!
//! Everything here runs on Linux with the CLAP and VST3 test plug-ins (`tests/plugins.rs`).
//! Type-checked only, for `aarch64-apple-darwin`: the short socket directory chosen on macOS
//! ([`manager::default_socket_dir`]), Audio Unit identities (`au:<type>:<subtype>:<manufacturer>`,
//! which need no file), editor windows (the container opens them; on Linux only CLAP's
//! floating windows exist) and yielding activation to the container before an editor is
//! focused ([`manager::yield_activation`]).

pub mod catalog;
pub mod ident;
pub mod insert;
pub mod manager;
pub mod recovery;
pub mod testing;

pub use catalog::{Catalog, CatalogOptions, ScanProgress};
pub use ident::Identity;
pub use insert::{PluginFactory, PluginInsert};
pub use manager::{Hosted, Lookup, Placement, PluginEvent, PluginManager, PluginOptions};
pub use recovery::RecoveryStore;
/// The container client (`crates/plugin-host/crates/client`).
pub use stpc_client as client;

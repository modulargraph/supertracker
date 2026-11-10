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
//!     Plan::engine_event .................. note events of a plug-in instrument's track -> PluginInsert::midi
//!     Plan::process ....................... automation and step locks -> PluginInsert::parameter_at(frame)
//!                                           PluginInsert::process -> Instance::process_raw (one round trip)
//!  PluginManager's thread ................. container events: lost, latency, restart, editor edits
//!     PluginEvent::Graph -> AudioEngine ... forgets the insert's processor and compiles again (delay compensation)
//! ```
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

//! The areas of the host beyond the session: each module registers the commands of one crate
//! (`docs/RUST_PORT_STATUS.md` lists them) and delegates to it.

pub mod audio;
pub mod export;
pub mod recording;
pub mod samples;
pub mod agent;
pub mod library;
pub mod midi;
pub mod mk3;
pub mod plugins;
pub mod services;

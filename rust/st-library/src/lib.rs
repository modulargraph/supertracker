//! SuperTracker's sound library: index and search, NKS metadata and presets, kits, Maschine group import. See docs/RUST_ARCHITECTURE.md.

pub mod api;
mod error;
mod index;
pub mod infer;
pub mod kit;
mod library;
pub mod maschine;
pub mod nks;
pub mod probe;
pub mod roots;
mod search;
pub mod util;

pub use error::{Error, Result};
pub use library::{EventSink, Library, LibraryConfig};

//! SuperTracker's sound library: index and search, NKS metadata and presets, kits, Maschine group import. See docs/RUST_ARCHITECTURE.md.

mod error;
pub mod infer;
pub mod kit;
pub mod nks;
pub mod probe;
pub mod util;

pub use error::{Error, Result};

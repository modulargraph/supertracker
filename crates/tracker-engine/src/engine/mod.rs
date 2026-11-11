//! SuperTracker's sequencer and voices.
//!
//! [`Engine::new`] creates an engine; [`host`] has its handles and the reasons they may cross
//! threads; `docs/AUDIO_ENGINE.md` describes what the engine plays.
//!
//! # Threading model
//!
//! * One serial **control thread** (the holder of the [`host::ControlHalf`]) edits a draft
//!   [`project::Project`] and publishes a complete immutable copy through an atomic pointer
//!   (`begin_update` / setters / `commit`).
//! * The **render thread** (the holder of the [`host::RenderHalf`]) adopts the latest project at
//!   the next block boundary, consumes transport and note commands from a bounded
//!   single-producer/single-consumer ring, and never blocks, locks, allocates or frees. Replaced
//!   projects go onto a lock-free retired list.
//! * `collect()` on the control thread destroys retired projects and the samples nothing refers
//!   to any more.
//! * Status, peaks and the voice playhead snapshot are atomics / seqlocks readable anywhere
//!   ([`host::StatusHandle`]).
//!
//! # Modules
//!
//! | module | holds |
//! |---|---|
//! | [`host`] | [`Engine`] and its handles: `ControlHalf`, `RenderHalf`, `StatusHandle`, `BlockView` |
//! | [`abi`] | the plain data and constants of the API (`STStep`, `STEvent`, `STStatus`, `ST_*`), bus arithmetic |
//! | [`edits`] | the control thread's operations: project setters and command producers (methods of `Control`) |
//! | [`control`] | draft/commit/collect, the render tables of a project, engine construction and destruction |
//! | [`transport`] | project adoption, Play/Stop/Seek, the musical clock, loops, queued jumps, input acks, checkpoints |
//! | [`sequencer`] | row and step triggering, step effects, ratchets, plug-in locks, automation lanes |
//! | [`voice`] | note start and end, and all voice DSP |
//! | [`mixer`] | the block renderer: channel mix, sends (delay/reverb), metronome, timeline clips |
//! | [`project`] | samples, zones, instruments, steps, lanes, patterns, the project |
//! | [`tables`] | per-channel render tables with the per-bus block buffers, peak store |
//! | [`state`] | the data of the two halves and of what they share; `Voice`, `Scheduled`, `Command` |
//! | [`util`] | `clamp`/`cmin`/`cmax`, `Ring`, `FloatBlock`, the random generator |
//!
//! The modules below `host` and `abi` are public because the mixer crate uses a few of their
//! building blocks (`util::Ring`, `state::{Comb, Allpass}`, `tables::MAX_BLOCK`, `edits::Pcm`)
//! and calls the methods of `Control`; the state they hold is private to the crate.

// Style lints this module deliberately does not follow:
// * Range checks are written as explicit comparisons (`x < low || x > high`). For floats that is
//   not a negated `contains`: a NaN passes the comparisons and fails `contains`, and several
//   setters rely on exactly that (the NaN is replaced by a default afterwards). The integer
//   checks keep the same shape so the two read alike.
// * The same goes for negated comparisons (`!(a < b)`), which are the NaN-aware form.
// * The setters take one argument per value of the thing they set; the mixer crate calls them
//   with these signatures.
// * `Vec<Box<Sample>>` and `Vec<Box<PeakStore>>`: the boxes give every sample and peak store a
//   stable address, which the render side holds on to while the vectors grow.
#![allow(
    clippy::manual_range_contains,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::too_many_arguments,
    clippy::vec_box,
    clippy::int_plus_one
)]

pub mod abi;
pub mod control;
pub mod edits;
pub mod host;
pub mod mixer;
pub mod project;
pub mod sequencer;
pub mod state;
pub mod tables;
pub mod transport;
pub mod util;
pub mod voice;

#[cfg(test)]
mod tests;

pub use abi::*;
pub use host::Engine;


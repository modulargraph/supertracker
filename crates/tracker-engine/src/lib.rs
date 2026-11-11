//! SuperTracker's audio engine: sequencer, sampler and synth voices, the built-in effects and
//! drum models. Pure DSP and timing: no I/O, no dependencies, no C interface.
//!
//! | module | what it is | start at |
//! |---|---|---|
//! | [`engine`] | the sequencer and its voices, rendered per track bus | [`engine::Engine`] |
//! | [`effects`] | the built-in effect catalogue, effect instances, drum models | [`effects::Effect`], [`effects::catalogue`] |
//!
//! Each module is behind a cargo feature of the same name (all on by default), so one part can
//! be built and tested without the others.
//!
//! Real-time rule for the whole crate: whatever is called from an audio callback (rendering,
//! effect processing) does not allocate, lock, block or free.
//! The integration tests enforce it with a counting allocator.

#[cfg(feature = "effects")]
pub mod effects;
#[cfg(feature = "engine")]
pub mod engine;

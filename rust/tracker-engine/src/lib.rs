//! SuperTracker's audio engine: sequencer, sampler and synth voices, the musical clock, the
//! built-in effects and drum models. Pure DSP and timing: no I/O, no dependencies, no C interface.
//!
//! | module | what it is | start at |
//! |---|---|---|
//! | [`engine`] | the sequencer and its voices, rendered per track bus | [`engine::Engine`] |
//! | [`clock`] | the sample-accurate tempo/meter clock | [`clock::ClockMap`], [`clock::ClockTimeline`] |
//! | [`effects`] | the built-in effect catalogue, effect instances, drum models | [`effects::Effect`], [`effects::catalogue`] |
//! | [`unit_range`] | a bounded render cache for one hosted unit (a plug-in) | [`unit_range::UnitRangeCache`] |
//!
//! Each module is behind a cargo feature of the same name (all on by default), so one part can
//! be built and tested without the others; `engine` needs `clock`.
//!
//! Real-time rule for the whole crate: whatever is called from an audio callback (rendering,
//! effect processing, clock queries, cache renders) does not allocate, lock, block or free.
//! The integration tests enforce it with a counting allocator.

#[cfg(feature = "clock")]
pub mod clock;
#[cfg(feature = "effects")]
pub mod effects;
#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "unit-range")]
pub mod unit_range;

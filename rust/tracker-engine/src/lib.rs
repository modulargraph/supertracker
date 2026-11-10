//! SuperTracker's audio engine.
//!
//! The crate exports exactly the C functions declared in `Sources/TrackerEngine/include`
//! (`TrackerEngine.h`, `TrackerClock.h`, `TrackerUnitRange.h`, `STBuiltinEffects.h`); those headers
//! are the contract with the Swift host and with the C++ test programs in `tests/`.
//!
//! Each header's implementation is a module behind a cargo feature of the same name, so one part
//! can be built and tested without the others.

#[cfg(feature = "cxx-alloc")]
mod cxx_alloc;

#[cfg(feature = "clock")]
pub mod clock;
#[cfg(feature = "effects")]
pub mod effects;
#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "unit-range")]
pub mod unit_range;

//! Built-in effects and drum synthesis: the implementation behind
//! `Sources/TrackerEngine/include/STBuiltinEffects.h`.
//!
//! First-party processors inspired by conventional studio effects. Effect ids and parameter
//! ids are persistent (saved projects and the Swift host refer to them); catalogue indices
//! are not project identities. All ranges and defaults are in displayed physical units.
//!
//! This is a faithful translation of the C++ sources it replaces, one module per file:
//!
//! | module         | replaces                                   |
//! |----------------|--------------------------------------------|
//! | [`capi`]       | `BuiltinEffects.cpp` (catalogue and C API) |
//! | [`common`]     | `BuiltinEffectsCommon.hpp`                 |
//! | [`dynamics`]   | `EffectsDynamics.hpp`                      |
//! | [`filtering`]  | `EffectsFiltering.hpp`                     |
//! | [`modulation`] | `EffectsModulation.hpp`                    |
//! | [`spatial`]    | `EffectsSpatial.hpp`                       |
//! | [`delay`]      | `EffectsDelay.hpp`                         |
//! | [`distortion`] | `EffectsDistortion.hpp`                    |
//! | [`perform`]    | `EffectsPerform.hpp`                       |
//! | [`drums`]      | `MaschineDrums.hpp`                        |
//!
//! Real-time rules: `Processor::process`/`reset` and `drums::Voice::tick` never allocate,
//! lock or block; every buffer is allocated when the processor is created.
//!
//! Numerics: the same `f32`/`f64` types and the same order of floating-point operations as
//! the C++, so on a given machine and libm the output is bit-identical to it (see
//! `common` for the few places where a libm call is spelled differently to match what the
//! C++ compiler emits).

// The numeric literals are the C++ ones, digit for digit (several are deliberately truncated
// approximations of well-known constants and must not be "corrected"), and the index loops
// mirror the C++ loops they translate.
#![allow(
    clippy::approx_constant,
    clippy::excessive_precision,
    clippy::needless_range_loop,
    clippy::manual_range_contains
)]

pub mod capi;
pub mod common;
pub mod delay;
pub mod distortion;
pub mod drums;
pub mod dynamics;
pub mod filtering;
pub mod modulation;
pub mod perform;
pub mod spatial;

pub use capi::{catalogue, find, STBuiltinEffect, STBuiltinEffectInfo, STBuiltinParameterInfo};
pub use common::{Core, Descriptor, EffectInfo, ParameterInfo, Processor};

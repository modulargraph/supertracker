//! Built-in effects and drum synthesis.
//!
//! First-party processors inspired by conventional studio effects. Effect ids and parameter
//! ids are persistent (saved projects refer to them); catalogue indices are not project
//! identities. All ranges and defaults are in displayed physical units.
//!
//! | module         | holds                                                              |
//! |----------------|--------------------------------------------------------------------|
//! | (this one)     | the catalogue: [`catalogue`], [`find`], the parameter mapping      |
//! | [`handle`]     | [`Effect`]: an instance with parameter mailboxes, bypass and gating |
//! | [`common`]     | the [`Processor`] interface, metadata types and DSP building blocks |
//! | [`dynamics`], [`filtering`], [`modulation`], [`spatial`], [`delay`], [`distortion`], [`perform`] | the effect families |
//! | [`drums`]      | the drum models: voices, metadata and the offline one-hit render   |
//!
//! Real-time rules: `Processor::process`/`reset`, `Effect::process`/`reset` and
//! `drums::Voice::tick` never allocate, lock or block; every buffer is allocated when the
//! processor is created.
//!
//! Numerics: fixed `f32`/`f64` types and a fixed order of floating-point operations, so on a
//! given machine and libm the output is reproducible bit for bit (the engine's regression
//! tests compare hashes of it).

// The numeric literals are kept digit for digit (several are deliberately truncated
// approximations of well-known constants and must not be "corrected": the recorded output
// depends on them), and the index loops read as the algorithms are usually written.
#![allow(
    clippy::approx_constant,
    clippy::excessive_precision,
    clippy::needless_range_loop,
    clippy::manual_range_contains
)]

pub mod common;
pub mod delay;
pub mod distortion;
pub mod drums;
pub mod dynamics;
pub mod filtering;
pub mod handle;
pub mod modulation;
pub mod perform;
pub mod spatial;

pub use common::{Core, Descriptor, EffectInfo, ParameterInfo, Processor};
pub use handle::{Effect, EffectControls, Sidechain};
use std::ffi::CStr;

/// Most descriptors the catalogue lists; further ones would be ignored.
const CATALOGUE_CAPACITY: usize = 64;

/// The effect families in catalogue order. Catalogue indices are not project identities;
/// effect and parameter ids are.
fn families() -> [&'static [Descriptor]; 7] {
    [
        dynamics::descriptors(),
        filtering::descriptors(),
        modulation::descriptors(),
        spatial::descriptors(),
        delay::descriptors(),
        distortion::descriptors(),
        perform::descriptors(),
    ]
}

/// Every catalogue descriptor, in catalogue order.
pub fn catalogue() -> impl Iterator<Item = &'static Descriptor> {
    families().into_iter().flat_map(|family| family.iter()).take(CATALOGUE_CAPACITY)
}

/// Looks an effect up by its persistent id.
pub fn find(id: &CStr) -> Option<&'static Descriptor> {
    catalogue().find(|descriptor| descriptor.info.id() == id)
}

/// [`find`] for an id held as a Rust string.
pub fn find_str(id: &str) -> Option<&'static Descriptor> {
    catalogue().find(|descriptor| descriptor.info.id().to_bytes() == id.as_bytes())
}

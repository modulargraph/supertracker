//! The mixer graph around the engine.
//!
//! | Module | What it holds |
//! |---|---|
//! | [`spec`] | the mixer a project describes, as comparable values; host automation numbering |
//! | [`plan`] | the compiler and the immutable render plan it produces; gain, pan and solo rules |
//! | [`pdc`] | delay compensation: the latency plan and the delay line |
//! | [`automation`] | sample-accurate timed values for faders, route gains and insert parameters |
//! | [`insert`] | the insert seam ([`insert::InsertProcessor`]) and built-in effects behind it |
//! | [`sends`] | the built-in delay and reverb returns |
//! | [`meters`] | per-strip peak meters and their ballistics |
//! | [`render`] | the renderer: pulls the engine, runs the plan; the hand-over of plans between threads |

pub mod automation;
pub mod insert;
pub mod meters;
pub mod pdc;
pub mod plan;
pub mod render;
pub mod sends;
pub mod spec;

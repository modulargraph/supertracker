//! A built-in effect instance for `tests/effects_builtin.rs` and `tests/effects_builtin_perf.rs`:
//! `tracker_engine::effects::Effect` with the id and catalogue entry the tests print next to it.

use tracker_engine::effects::{Descriptor, Effect};
pub use tracker_engine::effects::{EffectControls as Controls, Sidechain};

/// An owned effect instance: the handle's one render owner.
pub struct Fx {
    pub id: String,
    pub info: &'static Descriptor,
    effect: Effect,
}

impl Fx {
    /// The effect `id` at `sample_rate` for blocks of at most `max_frames`; `None` for an unknown
    /// id or arguments the engine rejects.
    pub fn create(id: &str, sample_rate: f64, max_frames: u32) -> Option<Fx> {
        let effect = Effect::create(id, sample_rate, max_frames)?;
        Some(Fx { id: id.to_string(), info: effect.descriptor(), effect })
    }

    /// The any-thread controls.
    pub fn controls(&self) -> Controls {
        self.effect.controls()
    }

    pub fn set_parameter(&self, parameter: u32, normalized_value: f64) -> bool {
        self.effect.set_parameter(parameter, normalized_value)
    }

    pub fn parameter(&self, parameter: u32) -> f64 {
        self.effect.parameter(parameter)
    }

    pub fn set_bypass(&self, bypass: bool) {
        self.effect.set_bypass(bypass);
    }

    pub fn set_tempo(&self, bpm: f64) {
        self.effect.set_tempo(bpm);
    }

    pub fn tail_seconds(&self) -> f64 {
        self.effect.tail_seconds()
    }

    pub fn latency(&self) -> u32 {
        self.effect.latency()
    }

    /// Clears the effect's memory (render owner only).
    pub fn reset(&mut self) {
        self.effect.reset();
    }

    /// In-place stereo render of `left.len()` frames; true unless the buffers are refused.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], sidechain: Sidechain) -> bool {
        assert_eq!(left.len(), right.len());
        self.effect.process(left, right, sidechain)
    }
}

//! A built-in effect instance for `tests/effects_builtin.rs` and `tests/effects_builtin_perf.rs`.
//!
//! The catalogue has a safe Rust API (`effects::catalogue`, `find`, `physical`, `normalized`,
//! `ParameterInfo`), which the tests use directly. The effect *handle* does not:
//!
//! TODO(api): `STBuiltinEffect` (parameter mailboxes, bypass, tempo, latency, tail estimate,
//! silence gating, denormal flushing, input clean-up) can only be created and driven through the
//! `st_builtin_effect_*` C functions; its constructor is private and it has no public methods.
//! Every `unsafe` block in this file is one of those calls. When the handle gets Rust methods
//! (`create`, `set_parameter`, `parameter`, `set_bypass`, `set_tempo`, `reset`, `latency`,
//! `tail_seconds`, `process` on slices, and a way to share the atomic controls with another
//! thread while one owner renders), only this file changes.

use std::ffi::CString;
use tracker_engine::effects::capi::*;
use tracker_engine::effects::{find, Descriptor};

/// The sidechain of one `process` call (the C function takes two optional pointers).
#[derive(Clone, Copy)]
pub enum Sidechain<'a> {
    /// `nullptr, nullptr`
    None,
    /// `left, nullptr`: one key channel (the C++ `FX::run(l, r, &sc)`).
    Left(&'a [f32]),
    /// `left, right`
    Both(&'a [f32], &'a [f32]),
    /// The main input buffers themselves (`l.data(), r.data()`): the key aliases the audio.
    MainInput,
}

/// An owned effect instance (`st_builtin_effect_create` / `st_builtin_effect_destroy`): the
/// handle's one render owner.
pub struct Fx {
    pub id: String,
    pub info: &'static Descriptor,
    raw: *mut STBuiltinEffect,
}

// SAFETY: the handle is plain owned state (its processor is `Send`); moving the owner to another
// thread moves the render ownership with it.
unsafe impl Send for Fx {}

/// The controls of an effect that may be used from any thread while its owner renders (header:
/// "atomic set/get/bypass/tempo/tail may overlap process").
#[derive(Clone, Copy)]
pub struct Controls {
    raw: *mut STBuiltinEffect,
}

// SAFETY: only the handle's atomic mailboxes are touched through `Controls` (the threading
// contract quoted above). The test keeps the owning `Fx` alive for as long as a `Controls` is
// used.
unsafe impl Send for Controls {}

impl Controls {
    /// `st_builtin_effect_set_parameter`
    pub fn set_parameter(&self, parameter: u32, normalized_value: f64) -> bool {
        // SAFETY: a live handle (see `Fx::controls`).
        unsafe { st_builtin_effect_set_parameter(self.raw, parameter, normalized_value) == 1 }
    }

    /// `st_builtin_effect_get_parameter`
    pub fn parameter(&self, parameter: u32) -> f64 {
        // SAFETY: a live handle.
        unsafe { st_builtin_effect_get_parameter(self.raw, parameter) }
    }

    /// `st_builtin_effect_set_bypass`
    pub fn set_bypass(&self, bypass: bool) {
        // SAFETY: a live handle.
        unsafe { st_builtin_effect_set_bypass(self.raw, bypass as i32) }
    }

    /// `st_builtin_effect_set_tempo`
    pub fn set_tempo(&self, bpm: f64) {
        // SAFETY: a live handle.
        unsafe { st_builtin_effect_set_tempo(self.raw, bpm) }
    }

    /// `st_builtin_effect_tail_seconds`
    pub fn tail_seconds(&self) -> f64 {
        // SAFETY: a live handle.
        unsafe { st_builtin_effect_tail_seconds(self.raw) }
    }
}

impl Fx {
    /// `st_builtin_effect_create(id, sample_rate, max_frames)`; `None` for an unknown id or
    /// arguments the function rejects.
    pub fn create(id: &str, sample_rate: f64, max_frames: u32) -> Option<Fx> {
        let name = CString::new(id).ok()?;
        // SAFETY: a valid NUL-terminated id.
        let raw = unsafe { st_builtin_effect_create(name.as_ptr(), sample_rate, max_frames) };
        if raw.is_null() {
            return None;
        }
        let info = find(&name).expect("a created effect is in the catalogue");
        Some(Fx { id: id.to_string(), info, raw })
    }

    /// The any-thread controls. They must not be used after this `Fx` is dropped.
    pub fn controls(&self) -> Controls {
        Controls { raw: self.raw }
    }

    pub fn set_parameter(&self, parameter: u32, normalized_value: f64) -> bool {
        self.controls().set_parameter(parameter, normalized_value)
    }

    pub fn parameter(&self, parameter: u32) -> f64 {
        self.controls().parameter(parameter)
    }

    pub fn set_bypass(&self, bypass: bool) {
        self.controls().set_bypass(bypass);
    }

    pub fn set_tempo(&self, bpm: f64) {
        self.controls().set_tempo(bpm);
    }

    pub fn tail_seconds(&self) -> f64 {
        self.controls().tail_seconds()
    }

    /// `st_builtin_effect_latency`
    pub fn latency(&self) -> u32 {
        // SAFETY: a live handle.
        unsafe { st_builtin_effect_latency(self.raw) }
    }

    /// `st_builtin_effect_reset` (render owner only).
    pub fn reset(&mut self) {
        // SAFETY: a live handle; `&mut self` is the render owner, so no process overlaps.
        unsafe { st_builtin_effect_reset(self.raw) }
    }

    /// `st_builtin_effect_process(effect, left, right, sidechain..., frames)` with
    /// `frames == left.len()`; true for the function's `1`.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], sidechain: Sidechain) -> bool {
        assert_eq!(left.len(), right.len());
        let frames = left.len() as u32;
        // The main pointers first: the aliasing sidechain form is derived from them.
        let (main_left, main_right) = (left.as_mut_ptr(), right.as_mut_ptr());
        let (side_left, side_right) = match sidechain {
            Sidechain::None => (std::ptr::null(), std::ptr::null()),
            Sidechain::Left(l) => {
                assert!(l.len() >= frames as usize);
                (l.as_ptr(), std::ptr::null())
            }
            Sidechain::Both(l, r) => {
                assert!(l.len() >= frames as usize && r.len() >= frames as usize);
                (l.as_ptr(), r.as_ptr())
            }
            Sidechain::MainInput => (main_left as *const f32, main_right as *const f32),
        };
        // SAFETY: a live handle whose render owner is `&mut self`; the buffers are valid for
        // `frames` samples. The function copies the sidechain before it writes the outputs,
        // which is what makes the aliasing `MainInput` form part of its contract.
        unsafe { st_builtin_effect_process(self.raw, main_left, main_right, side_left, side_right, frames) == 1 }
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `st_builtin_effect_create` and is not used again.
        unsafe { st_builtin_effect_destroy(self.raw) }
    }
}

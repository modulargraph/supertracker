//! The C++ `long double` used by the tempo prefix sum in `ClockTimingMap.cpp`.
//!
//! `compileTempos` accumulates segment start times in `long double` (with Kahan compensation) and
//! rounds each one to `double` once. What `long double` is depends on the target:
//!
//! * Apple Silicon: the same 64-bit format as `double`. The sum is then plain `f64` arithmetic.
//! * x86 and x86-64 (Intel Macs, and the Linux machines that run the test programs): the x87
//!   80-bit double-extended format, with a 64-bit significand.
//!
//! Rust has no 80-bit float, so [`super::x87::Extended`] implements the four operations the sum needs in
//! software, correctly rounded to nearest-even exactly as the x87 does. This keeps every compiled
//! tempo boundary bit-identical to the C++ engine on both architectures: one more or one fewer
//! unit in the last place of a boundary can move an event by a whole sample.

use std::ops::{Add, Div, Mul, Sub};

/// The arithmetic `compileTempos` performs in `long double`.
pub(crate) trait LongDouble:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self>
{
    /// `static_cast<long double>(value)`: always exact.
    fn from_f64(value: f64) -> Self;
    /// `static_cast<double>(value)`: one rounding to nearest-even.
    fn to_f64(self) -> f64;
}

impl LongDouble for f64 {
    #[inline]
    fn from_f64(value: f64) -> Self {
        value
    }
    #[inline]
    fn to_f64(self) -> f64 {
        self
    }
}

/// `long double` of the C++ compiler for the target being built.
#[cfg(all(
    any(target_arch = "x86", target_arch = "x86_64"),
    not(target_env = "msvc")
))]
pub(crate) type CxxLongDouble = super::x87::Extended;
/// `long double` of the C++ compiler for the target being built.
#[cfg(not(all(
    any(target_arch = "x86", target_arch = "x86_64"),
    not(target_env = "msvc")
)))]
pub(crate) type CxxLongDouble = f64;

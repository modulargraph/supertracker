//! The few `<algorithm>` / `<cmath>` functions whose exact semantics the clock relies on.
//!
//! `f64::min` / `f64::max` are not `std::min` / `std::max`: they differ for NaN and for zeros of
//! opposite sign. The clock compares against stored boundaries with these, so they are spelled
//! out here exactly as the C++ standard library defines them.

/// `std::min(a, b)`: `b` only when it is strictly smaller.
#[inline]
pub(crate) fn min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max(a, b)`: `b` only when it is strictly larger.
#[inline]
pub(crate) fn max(a: f64, b: f64) -> f64 {
    if a < b {
        b
    } else {
        a
    }
}

/// `std::nextafter(value, target)` for a finite `value` and any `target` below it: the next
/// representable double towards negative infinity (`-denorm_min` from either zero).
#[inline]
pub(crate) fn next_below(value: f64) -> f64 {
    value.next_down()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_max_follow_the_cxx_definitions() {
        assert_eq!(min(1.0, 2.0), 1.0);
        assert_eq!(min(2.0, 1.0), 1.0);
        assert_eq!(max(1.0, 2.0), 2.0);
        assert_eq!(max(2.0, 1.0), 2.0);
        // Equal values (including zeros of either sign) keep the first argument.
        assert!(min(0.0, -0.0).is_sign_positive());
        assert!(min(-0.0, 0.0).is_sign_negative());
        assert!(max(0.0, -0.0).is_sign_positive());
        assert!(max(-0.0, 0.0).is_sign_negative());
        // An unordered comparison is false, so the first argument wins.
        assert!(min(f64::NAN, 1.0).is_nan());
        assert_eq!(min(1.0, f64::NAN), 1.0);
        assert!(max(f64::NAN, 1.0).is_nan());
        assert_eq!(max(1.0, f64::NAN), 1.0);
    }

    #[test]
    fn next_below_matches_nextafter_downwards() {
        assert_eq!(next_below(2.0), f64::from_bits(2.0f64.to_bits() - 1));
        assert_eq!(next_below(1.0), 1.0 - f64::EPSILON / 2.0);
        assert_eq!(next_below(0.0), -f64::from_bits(1));
        assert_eq!(next_below(-0.0), -f64::from_bits(1));
        assert_eq!(next_below(-4.0), f64::from_bits((-4.0f64).to_bits() + 1));
    }
}

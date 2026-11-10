//! Software x87 double-extended arithmetic: the C++ `long double` of x86 targets.
//!
//! See [`super::long_double`] for why the clock needs it. Compiled where `long double` is the x87
//! format, and for tests on every target so the same reference values are checked everywhere.

use std::ops::{Add, Div, Mul, Sub};

use super::long_double::LongDouble;

/// A finite x87 double-extended value: `(-1)^negative * significand * 2^exponent`.
///
/// Only finite values are representable and the exponent is unbounded, which is exact for the
/// tempo sum: its operands are validated doubles (beats within 0..1000000, tempos within 20..400)
/// and every intermediate result lies between roughly 2^-1200 and 2^30, far inside the x87
/// exponent range of 2^-16445..2^16384. No overflow, underflow or NaN can occur there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Extended {
    negative: bool,
    /// Bit 63 is set, except for zero.
    significand: u64,
    /// Power of two of the least significant bit of `significand`.
    exponent: i32,
}

impl Extended {
    const fn zero(negative: bool) -> Self {
        Self {
            negative,
            significand: 0,
            exponent: 0,
        }
    }

    /// Rounds `magnitude * 2^exponent`, plus a positive excess below one unit of `magnitude`
    /// when `sticky` is set, to 64 significant bits (nearest, ties to even).
    fn rounded(negative: bool, magnitude: u128, exponent: i32, sticky: bool) -> Self {
        if magnitude == 0 {
            return Self::zero(negative);
        }
        let bits = 128 - magnitude.leading_zeros();
        if bits <= 64 {
            // Exact: callers only report an excess together with at least 65 significant bits.
            debug_assert!(!sticky);
            let shift = 64 - bits;
            return Self {
                negative,
                significand: (magnitude << shift) as u64,
                exponent: exponent - shift as i32,
            };
        }
        let shift = bits - 64;
        let mut significand = (magnitude >> shift) as u64;
        let mut exponent = exponent + shift as i32;
        let remainder = magnitude & ((1u128 << shift) - 1);
        let half = 1u128 << (shift - 1);
        if remainder > half || (remainder == half && (sticky || significand & 1 == 1)) {
            match significand.checked_add(1) {
                Some(next) => significand = next,
                None => {
                    significand = 1 << 63;
                    exponent += 1;
                }
            }
        }
        Self {
            negative,
            significand,
            exponent,
        }
    }

    #[cfg(test)]
    fn negated(self) -> Self {
        Self {
            negative: !self.negative,
            ..self
        }
    }

    /// The value as the x87 stores it: (sign and biased exponent, significand).
    #[cfg(test)]
    fn x87_encoding(self) -> (u16, u64) {
        let sign = (self.negative as u16) << 15;
        if self.significand == 0 {
            return (sign, 0);
        }
        let biased = self.exponent + 63 + 16383;
        assert!((1..32767).contains(&biased), "outside the x87 normal range");
        (sign | biased as u16, self.significand)
    }
}

impl LongDouble for Extended {
    fn from_f64(value: f64) -> Self {
        debug_assert!(value.is_finite());
        let bits = value.to_bits();
        let negative = bits >> 63 != 0;
        let biased = ((bits >> 52) & 0x7ff) as i32;
        let fraction = bits & ((1 << 52) - 1);
        let (significand, exponent) = if biased == 0 {
            (fraction, -1074)
        } else {
            (fraction | 1 << 52, biased - 1075)
        };
        Self::rounded(negative, significand as u128, exponent, false)
    }

    fn to_f64(self) -> f64 {
        let sign = (self.negative as u64) << 63;
        if self.significand == 0 {
            return f64::from_bits(sign);
        }
        let infinity = f64::from_bits(sign | 0x7ff << 52);
        // Power of two of the leading significand bit.
        let top = self.exponent + 63;
        if top > 1023 {
            return infinity;
        }
        // A normal double keeps 53 of the 64 bits. Below 2^-1022 the result is subnormal: its
        // last place is fixed at 2^-1074, so more bits go. Either way there is a single rounding.
        let discarded = if top >= -1022 {
            11
        } else {
            (-1074 - self.exponent) as u32
        };
        let rounded = if discarded > 64 {
            0 // Less than half of the smallest subnormal.
        } else {
            let (kept, remainder) = if discarded == 64 {
                (0, self.significand)
            } else {
                (
                    self.significand >> discarded,
                    self.significand & ((1 << discarded) - 1),
                )
            };
            let half = 1u64 << (discarded - 1);
            kept + (remainder > half || (remainder == half && kept & 1 == 1)) as u64
        };
        if top < -1022 {
            // A carry out of the subnormal range lands in the exponent field as 2^-1022.
            return f64::from_bits(sign | rounded);
        }
        let (fraction, top) = if rounded == 1 << 53 {
            (0, top + 1)
        } else {
            (rounded & ((1 << 52) - 1), top)
        };
        if top > 1023 {
            return infinity;
        }
        f64::from_bits(sign | ((top + 1023) as u64) << 52 | fraction)
    }
}

impl Add for Extended {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        if self.significand == 0 {
            // (+0) + (-0) is +0 when rounding to nearest; equal signs are kept.
            return if other.significand == 0 {
                Self::zero(self.negative && other.negative)
            } else {
                other
            };
        }
        if other.significand == 0 {
            return self;
        }
        // Both significands are normalized, so this orders by magnitude.
        let (large, small) =
            if (self.exponent, self.significand) >= (other.exponent, other.significand) {
                (self, other)
            } else {
                (other, self)
            };
        let distance = (large.exponent - small.exponent) as u32;
        // 63 guard bits below the larger operand; one spare bit above it for a carry.
        let wide_large = (large.significand as u128) << 63;
        let wide_small = (small.significand as u128) << 63;
        let (aligned, sticky) = if distance >= 127 {
            (0, true)
        } else {
            (
                wide_small >> distance,
                wide_small & ((1u128 << distance) - 1) != 0,
            )
        };
        let exponent = large.exponent - 63;
        if large.negative == small.negative {
            return Self::rounded(large.negative, wide_large + aligned, exponent, sticky);
        }
        // large - (aligned + excess) = (large - aligned - 1) + (1 - excess). Bits can only be
        // lost at a distance of 64 or more, where the difference stays far above zero.
        let magnitude = wide_large - aligned - sticky as u128;
        if magnitude == 0 {
            return Self::zero(false); // Exact cancellation is +0 when rounding to nearest.
        }
        Self::rounded(large.negative, magnitude, exponent, sticky)
    }
}

impl Sub for Extended {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        self + Self {
            negative: !other.negative,
            ..other
        }
    }
}

impl Mul for Extended {
    type Output = Self;
    fn mul(self, other: Self) -> Self {
        let negative = self.negative != other.negative;
        if self.significand == 0 || other.significand == 0 {
            return Self::zero(negative);
        }
        let product = self.significand as u128 * other.significand as u128;
        Self::rounded(negative, product, self.exponent + other.exponent, false)
    }
}

impl Div for Extended {
    type Output = Self;
    fn div(self, other: Self) -> Self {
        let negative = self.negative != other.negative;
        // The only divisor in the tempo sum is a validated tempo of at least 20, never zero.
        // An infinite quotient is not representable here, so do not divide rather than trap.
        debug_assert!(other.significand != 0);
        if self.significand == 0 || other.significand == 0 {
            return Self::zero(negative);
        }
        let divisor = other.significand as u128;
        let numerator = (self.significand as u128) << 64;
        // 64 or 65 quotient bits, then one more so that a rounding bit always exists.
        let quotient = numerator / divisor;
        let doubled = (numerator % divisor) << 1;
        let (bit, remainder) = if doubled >= divisor {
            (1, doubled - divisor)
        } else {
            (0, doubled)
        };
        Self::rounded(
            negative,
            quotient << 1 | bit,
            self.exponent - other.exponent - 65,
            remainder != 0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extended(value: f64) -> Extended {
        Extended::from_f64(value)
    }

    #[test]
    fn doubles_convert_exactly_both_ways() {
        let samples = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.1,
            1e6,
            123.456e-7,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::from_bits(0x000f_ffff_ffff_ffff),
            -f64::from_bits(0x0000_0000_0123_4567),
        ];
        for value in samples {
            assert_eq!(
                extended(value).to_f64().to_bits(),
                value.to_bits(),
                "{value:e}"
            );
        }
        assert_eq!(extended(1.0).x87_encoding(), (0x3fff, 1 << 63));
        assert_eq!(extended(-2.0).x87_encoding(), (0xc000, 1 << 63));
        assert_eq!(
            extended(f64::from_bits(1)).x87_encoding(),
            (16383 - 1074, 1 << 63)
        );
    }

    #[test]
    fn results_are_rounded_to_64_bits_nearest_even() {
        let one = extended(1.0);
        let ulp = Extended {
            negative: false,
            significand: 1 << 63,
            exponent: -126,
        }; // 2^-63
        let half_ulp = Extended {
            exponent: -127,
            ..ulp
        };
        let quarter_ulp = Extended {
            exponent: -128,
            ..ulp
        };
        // A tie rounds to the even significand; anything above the tie rounds up.
        assert_eq!(one + half_ulp, one);
        assert_eq!(
            (one + ulp) + half_ulp,
            Extended {
                negative: false,
                significand: (1 << 63) + 2,
                exponent: -63
            }
        );
        assert_eq!(one + (half_ulp + quarter_ulp), one + ulp);
        // Below one the spacing halves, so the same operands subtract exactly or tie to even.
        assert_eq!(
            one - half_ulp,
            Extended {
                negative: false,
                significand: u64::MAX,
                exponent: -64
            }
        );
        assert_eq!(one - quarter_ulp, one);
        assert_eq!(
            one - Extended {
                exponent: -200,
                ..ulp
            },
            one
        );
        assert_eq!(
            (one - Extended {
                exponent: -200,
                ..ulp
            })
            .negated()
                + one,
            Extended::zero(false)
        );
        // 1/3 and 2/3 in double-extended.
        assert_eq!(
            (one / extended(3.0)).x87_encoding(),
            (0x3ffd, 0xaaaa_aaaa_aaaa_aaab)
        );
        assert_eq!(
            (extended(2.0) / extended(3.0)).x87_encoding(),
            (0x3ffe, 0xaaaa_aaaa_aaaa_aaab)
        );
        // The remaining values were printed by an x87 (clang, x86-64 `long double`), with every
        // operand converted from the double nearest the decimal shown.
        let tenth = extended(0.1);
        assert_eq!(
            (tenth * tenth).x87_encoding(),
            (0x3ff8, 0xa3d7_0a3d_70a3_dc29)
        );
        assert_eq!(
            (tenth + tenth * tenth).x87_encoding(),
            (0x3ffb, 0xe147_ae14_7ae1_4b85)
        );
        assert_eq!(
            (one - tenth).x87_encoding(),
            (0x3ffe, 0xe666_6666_6666_6600)
        );
        assert_eq!(
            (tenth - one / extended(3.0)).x87_encoding(),
            (0xbffc, 0xeeee_eeee_eeee_ed56)
        );
        assert_eq!(
            (tenth * extended(60.0) / extended(3.0)).x87_encoding(),
            (0x4000, 0x8000_0000_0000_0200)
        );
        assert_eq!(
            (tenth + tenth * tenth).to_f64(),
            f64::from_bits(0x3fbc_28f5_c28f_5c29)
        );
        assert_eq!(
            (tenth - one / extended(3.0)).to_f64(),
            -f64::from_bits(0x3fcd_dddd_dddd_ddde)
        );
    }

    #[test]
    fn zeros_follow_ieee_sign_rules() {
        let zero = extended(0.0);
        let negative_zero = extended(-0.0);
        assert_eq!(zero + negative_zero, zero);
        assert_eq!(negative_zero + negative_zero, negative_zero);
        assert_eq!(extended(5.0) - extended(5.0), zero);
        assert_eq!(extended(-5.0) + extended(5.0), zero);
        assert_eq!(negative_zero * extended(3.0), negative_zero);
        assert_eq!(zero / extended(-3.0), negative_zero);
        assert_eq!(negative_zero + extended(3.0), extended(3.0));
    }

    #[test]
    fn narrowing_rounds_once_including_subnormals() {
        // 1 + 2^-53 is a tie between 1 and 1 + 2^-52: even wins. One bit more breaks the tie.
        let tie = extended(1.0)
            + Extended {
                negative: false,
                significand: 1 << 63,
                exponent: -116,
            };
        assert_eq!(tie.to_f64(), 1.0);
        let above = tie
            + Extended {
                negative: false,
                significand: 1 << 63,
                exponent: -126,
            };
        assert_eq!(above.to_f64(), 1.0 + f64::EPSILON);
        // Rounding up across a power of two.
        let below_two = Extended {
            negative: false,
            significand: u64::MAX,
            exponent: -63,
        };
        assert_eq!(below_two.to_f64(), 2.0);
        // Subnormal results: 2^-1075 is a tie to zero, anything above it is the smallest double.
        let tiny = Extended {
            negative: false,
            significand: 1 << 63,
            exponent: -1075 - 63,
        };
        assert_eq!(tiny.to_f64().to_bits(), 0);
        assert_eq!(
            Extended {
                significand: (1 << 63) + 1,
                ..tiny
            }
            .to_f64()
            .to_bits(),
            1
        );
        assert_eq!(
            Extended {
                exponent: -1076 - 63,
                ..tiny
            }
            .to_f64()
            .to_bits(),
            0
        );
        assert_eq!(
            Extended {
                negative: true,
                significand: 3 << 62,
                exponent: -1074 - 63
            }
            .to_f64()
            .to_bits(),
            1 << 63 | 2
        );
        // The largest subnormal plus half a unit ties up into the smallest normal double.
        let carry = Extended {
            negative: false,
            significand: u64::MAX << 11,
            exponent: -1023 - 63,
        };
        assert_eq!(carry.to_f64(), f64::MIN_POSITIVE);
        assert_eq!((extended(f64::MAX) * extended(2.0)).to_f64(), f64::INFINITY);
    }
}

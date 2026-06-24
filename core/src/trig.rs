//! Fixed-point trig + sqrt via an integer LUT and integer algorithms (invariant #1).
//! No std/libm transcendentals: the sine table is baked at build time (see
//! `build/lut.rs`) and `sqrt` is an integer isqrt. Angles use "binary radians" so a full
//! turn is a power of two and wrapping is a mask — angle math never drifts.

use crate::fixed::Fixed;

// Brings `SIN_LUT_LEN` (usize) and `SIN_LUT` ([i32; SIN_LUT_LEN], Q16.16 bits) into scope.
include!(concat!(env!("OUT_DIR"), "/lut_generated.rs"));

/// Bits of angle resolution. Full turn = `1 << ANGLE_BITS`.
pub const ANGLE_BITS: u32 = 16;
/// One full turn in angle units.
pub const ANGLE_FULL: i32 = 1 << ANGLE_BITS;

/// An angle in binary radians (full turn = [`ANGLE_FULL`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Angle(pub i32);

impl Angle {
    /// Reduce to `[0, ANGLE_FULL)` by masking (turn is a power of two).
    #[inline]
    pub const fn wrap(self) -> i32 {
        self.0 & (ANGLE_FULL - 1)
    }
}

/// Sine of an angle, as Fixed in `[-1, 1]`.
#[inline]
pub fn sin(a: Angle) -> Fixed {
    let idx = ((a.wrap() as i64 * SIN_LUT_LEN as i64) >> ANGLE_BITS) as usize;
    Fixed::from_bits(SIN_LUT[idx & (SIN_LUT_LEN - 1)])
}

/// Cosine of an angle, as Fixed in `[-1, 1]`.
#[inline]
pub fn cos(a: Angle) -> Fixed {
    sin(Angle(a.0.wrapping_add(ANGLE_FULL / 4)))
}

/// Square root of a non-negative Fixed via integer isqrt (negative input → zero).
///
/// `sqrt(x)` in real units is `isqrt(bits << FRAC_BITS)` in Q16.16 bits, because
/// `sqrt(b / 2^16) * 2^16 = isqrt(b << 16)`.
#[inline]
pub fn sqrt(x: Fixed) -> Fixed {
    let bits = x.to_bits();
    if bits <= 0 {
        return Fixed::ZERO;
    }
    let r = ((bits as u64) << Fixed::FRAC_BITS).isqrt();
    Fixed::from_bits(r as i32)
}

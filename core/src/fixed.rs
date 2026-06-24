//! Q16.16 fixed-point — the sim's only scalar type (invariant #1, decisions.md D17).
//!
//! There is deliberately NO conversion to or from the floating types in this module: a
//! float cannot enter the sim because the type simply does not implement it. The renderer
//! converts at its own boundary via [`Fixed::to_bits`]. This makes "no floats in the sim"
//! a *compile error*, not a convention. Arithmetic uses explicit wrapping so debug,
//! release, and every target arch produce bit-identical results.

use core::fmt;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Signed Q16.16 fixed-point number. 16 integer bits, 16 fractional bits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fixed(i32);

impl Fixed {
    pub const FRAC_BITS: u32 = 16;
    pub const SCALE: i32 = 1 << Self::FRAC_BITS;

    pub const ZERO: Fixed = Fixed(0);
    pub const ONE: Fixed = Fixed(Self::SCALE);
    pub const HALF: Fixed = Fixed(Self::SCALE / 2);
    pub const MAX: Fixed = Fixed(i32::MAX);
    pub const MIN: Fixed = Fixed(i32::MIN);

    /// Reinterpret raw Q16.16 bits. The renderer's only sanctioned entry point.
    #[inline]
    pub const fn from_bits(bits: i32) -> Self {
        Fixed(bits)
    }

    /// Raw Q16.16 bits. The renderer converts these to a float at its boundary.
    #[inline]
    pub const fn to_bits(self) -> i32 {
        self.0
    }

    /// Whole integer → Fixed.
    #[inline]
    pub const fn from_int(n: i32) -> Self {
        Fixed(n.wrapping_shl(Self::FRAC_BITS))
    }

    /// Truncate toward negative infinity (arithmetic shift) to a whole integer.
    #[inline]
    pub const fn to_int(self) -> i32 {
        self.0 >> Self::FRAC_BITS
    }

    /// Exact rational `num/den` as Fixed — the float-free way to write a constant.
    #[inline]
    pub const fn from_ratio(num: i32, den: i32) -> Self {
        Fixed(((num as i64 * Self::SCALE as i64) / den as i64) as i32)
    }

    #[inline]
    pub const fn wrapping_add(self, o: Self) -> Self {
        Fixed(self.0.wrapping_add(o.0))
    }

    #[inline]
    pub const fn wrapping_sub(self, o: Self) -> Self {
        Fixed(self.0.wrapping_sub(o.0))
    }

    /// Fixed-point multiply via an i64 intermediate (no precision loss before the shift).
    #[inline]
    pub const fn mul_fixed(self, o: Self) -> Self {
        Fixed(((self.0 as i64 * o.0 as i64) >> Self::FRAC_BITS) as i32)
    }

    /// Fixed-point divide via an i64 intermediate. Divisor must be non-zero (a zero
    /// divisor panics deterministically — a logic error, not a desync source).
    #[inline]
    pub const fn div_fixed(self, o: Self) -> Self {
        Fixed((((self.0 as i64) << Self::FRAC_BITS) / o.0 as i64) as i32)
    }

    #[inline]
    pub const fn abs(self) -> Self {
        Fixed(self.0.wrapping_abs())
    }

    #[inline]
    pub const fn signum(self) -> Self {
        if self.0 > 0 {
            Fixed::ONE
        } else if self.0 < 0 {
            Fixed(-Fixed::SCALE)
        } else {
            Fixed::ZERO
        }
    }

    #[inline]
    pub const fn min(self, o: Self) -> Self {
        if self.0 <= o.0 {
            self
        } else {
            o
        }
    }

    #[inline]
    pub const fn max(self, o: Self) -> Self {
        if self.0 >= o.0 {
            self
        } else {
            o
        }
    }
}

impl Add for Fixed {
    type Output = Fixed;
    #[inline]
    fn add(self, o: Self) -> Fixed {
        self.wrapping_add(o)
    }
}

impl Sub for Fixed {
    type Output = Fixed;
    #[inline]
    fn sub(self, o: Self) -> Fixed {
        self.wrapping_sub(o)
    }
}

impl Mul for Fixed {
    type Output = Fixed;
    #[inline]
    fn mul(self, o: Self) -> Fixed {
        self.mul_fixed(o)
    }
}

impl Div for Fixed {
    type Output = Fixed;
    #[inline]
    fn div(self, o: Self) -> Fixed {
        self.div_fixed(o)
    }
}

impl Neg for Fixed {
    type Output = Fixed;
    #[inline]
    fn neg(self) -> Fixed {
        Fixed(self.0.wrapping_neg())
    }
}

impl AddAssign for Fixed {
    #[inline]
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl SubAssign for Fixed {
    #[inline]
    fn sub_assign(&mut self, o: Self) {
        *self = *self - o;
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print int part and raw fractional bits — never a float.
        write!(
            f,
            "Fixed({}+{}/{})",
            self.0 >> Self::FRAC_BITS,
            self.0 & (Self::SCALE - 1),
            Self::SCALE
        )
    }
}

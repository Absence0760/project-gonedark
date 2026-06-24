//! Sim components — plain data, fixed-point only. Stored struct-of-arrays in the ECS.

use crate::fixed::Fixed;
use crate::trig;
use core::ops::{Add, Sub};

/// 2D vector in Q16.16 world units.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Vec2 {
    pub x: Fixed,
    pub y: Fixed,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 {
        x: Fixed::ZERO,
        y: Fixed::ZERO,
    };

    #[inline]
    pub const fn new(x: Fixed, y: Fixed) -> Self {
        Vec2 { x, y }
    }

    #[inline]
    pub fn scale(self, s: Fixed) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }

    #[inline]
    pub fn dot(self, o: Vec2) -> Fixed {
        self.x * o.x + self.y * o.y
    }

    /// Squared length (cheap; no sqrt). Prefer this for distance comparisons.
    #[inline]
    pub fn len_sq(self) -> Fixed {
        self.dot(self)
    }

    /// Length via fixed-point sqrt.
    #[inline]
    pub fn len(self) -> Fixed {
        trig::sqrt(self.len_sq())
    }

    /// Unit vector; a zero vector returns zero (never divides by zero).
    #[inline]
    pub fn normalized(self) -> Vec2 {
        let l = self.len();
        if l == Fixed::ZERO {
            Vec2::ZERO
        } else {
            Vec2::new(self.x / l, self.y / l)
        }
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

/// A unit's current order. The literal executor (invariant #3) holds exactly this and
/// does it — no autonomy. The full order/stance *vocabulary* is Phase 2; this is the seam.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Order {
    #[default]
    Idle,
    MoveTo(Vec2),
}

/// A unit's engagement stance (stubbed vocabulary for Phase 1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stance {
    HoldFire,
    #[default]
    ReturnFire,
    FireAtWill,
}

/// Where a unit's input comes from (invariant #5, D6/D7). `Orders` = command layer /
/// literal-executor AI; `Embodied` = live player input while possessed. Flipping this is
/// the *entirety* of possession — there is no separate character object and no respawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InputSource {
    #[default]
    Orders,
    Embodied,
}

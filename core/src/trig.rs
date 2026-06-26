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

/// Angle (binary radians) of the vector `(x, y)` — the fixed-point `atan2`. Returns the `Angle`
/// whose direction `(cos, sin)` points along `(x, y)`, in `[0, ANGLE_FULL)`. Float-free: an
/// octant reduction (sign + smaller/larger swap) maps any vector into the first octant, the
/// baked [`ATAN_LUT`] supplies `atan(min/max)` there, and the base angle is reflected into the
/// correct quadrant by the component signs. The zero vector has no direction; it returns
/// `Angle(0)` by convention (callers gate on a non-zero aim before slewing).
///
/// Convention matches [`sin`]/[`cos`]: `+X` is `0`, angle increases counter-clockwise toward
/// `+Y` (a quarter turn = `ANGLE_FULL/4`).
#[inline]
pub fn atan2(y: Fixed, x: Fixed) -> Angle {
    let ax = x.abs();
    let ay = y.abs();
    // Base angle in the first octant: atan(smaller / larger) ∈ [0, ANGLE_FULL/8]. `swapped`
    // means |y| > |x|, i.e. the vector is steeper than 45°, so the looked-up angle is measured
    // from the Y axis and we take its complement (ANGLE_FULL/4 − base) to measure from +X.
    let (smaller, larger, swapped) = if ay <= ax { (ay, ax, false) } else { (ax, ay, true) };
    let base = if larger == Fixed::ZERO {
        0 // zero vector: ratio undefined → angle 0 (with the sign reflection below, still 0).
    } else {
        let ratio = smaller / larger; // ∈ [0, 1] as Fixed (Q16.16 bits ∈ [0, 65536]).
        let idx = ((ratio.to_bits() as i64 * (ATAN_LUT_LEN as i64 - 1)) >> ANGLE_BITS) as usize;
        ATAN_LUT[idx.min(ATAN_LUT_LEN - 1)]
    };
    let first_quadrant = if swapped { (ANGLE_FULL / 4) - base } else { base };
    // Reflect the first-quadrant angle into the vector's actual quadrant by component signs.
    let a = match (x.to_bits() >= 0, y.to_bits() >= 0) {
        (true, true) => first_quadrant,                 // Q1: [0, 90°)
        (false, true) => ANGLE_FULL / 2 - first_quadrant, // Q2: (90°, 180°]
        (false, false) => ANGLE_FULL / 2 + first_quadrant, // Q3: (180°, 270°]
        (true, false) => ANGLE_FULL - first_quadrant,   // Q4: (270°, 360°)
    };
    Angle(a & (ANGLE_FULL - 1))
}

/// Rotate `from` toward `target` by at most `max_step` angle-units, taking the **shortest way
/// around** the wrap seam, and snapping exactly onto `target` once within a step. The turret /
/// hull slew primitive: an angular speed limiter. `max_step` is clamped to non-negative; a
/// `max_step` of `0` leaves `from` unchanged, and a value at or above a half-turn always reaches
/// `target` in one call. Pure integer arithmetic — deterministic on every peer (invariant #1).
#[inline]
pub fn rotate_toward(from: Angle, target: Angle, max_step: i32) -> Angle {
    let step = max_step.max(0);
    // Signed shortest delta in (−ANGLE_FULL/2, ANGLE_FULL/2].
    let raw = (target.0 - from.0) & (ANGLE_FULL - 1);
    let delta = if raw > ANGLE_FULL / 2 { raw - ANGLE_FULL } else { raw };
    if delta.abs() <= step {
        Angle(target.wrap())
    } else if delta > 0 {
        Angle((from.0 + step) & (ANGLE_FULL - 1))
    } else {
        Angle((from.0 - step) & (ANGLE_FULL - 1))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    /// Shortest signed angular difference `a − b`, in `(−ANGLE_FULL/2, ANGLE_FULL/2]`.
    fn shortest_diff(a: Angle, b: Angle) -> i32 {
        let raw = (a.0 - b.0) & (ANGLE_FULL - 1);
        if raw > ANGLE_FULL / 2 {
            raw - ANGLE_FULL
        } else {
            raw
        }
    }

    // --- atan2 -------------------------------------------------------------------------------

    #[test]
    fn atan2_cardinal_directions_are_exact() {
        assert_eq!(atan2(fx(0), fx(1)).0, 0, "+X = 0");
        assert_eq!(atan2(fx(1), fx(0)).0, ANGLE_FULL / 4, "+Y = quarter turn");
        assert_eq!(atan2(fx(0), fx(-1)).0, ANGLE_FULL / 2, "-X = half turn");
        assert_eq!(atan2(fx(-1), fx(0)).0, 3 * ANGLE_FULL / 4, "-Y = three-quarter turn");
    }

    #[test]
    fn atan2_diagonals_are_exact_eighths() {
        // atan(1) = 45° sits exactly on the last LUT entry, so the diagonals land on exact eighths.
        assert_eq!(atan2(fx(1), fx(1)).0, ANGLE_FULL / 8, "Q1 diagonal = 45°");
        assert_eq!(atan2(fx(1), fx(-1)).0, 3 * ANGLE_FULL / 8, "Q2 diagonal = 135°");
        assert_eq!(atan2(fx(-1), fx(-1)).0, 5 * ANGLE_FULL / 8, "Q3 diagonal = 225°");
        assert_eq!(atan2(fx(-1), fx(1)).0, 7 * ANGLE_FULL / 8, "Q4 diagonal = 315°");
    }

    #[test]
    fn atan2_magnitude_invariant() {
        // Direction only depends on the ratio, not the magnitude.
        assert_eq!(atan2(fx(3), fx(3)), atan2(fx(100), fx(100)));
        assert_eq!(atan2(fx(7), fx(0)), atan2(fx(1), fx(0)));
    }

    #[test]
    fn atan2_zero_vector_is_zero_by_convention() {
        assert_eq!(atan2(fx(0), fx(0)).0, 0);
    }

    #[test]
    fn atan2_steep_vs_shallow_use_the_swap_branch() {
        // A shallow vector (|y| < |x|) and its mirror across the 45° line are complementary.
        let shallow = atan2(fx(1), fx(2)).0; // ~26 degrees
        let steep = atan2(fx(2), fx(1)).0; // ~63 degrees (the complement)
        assert_eq!(shallow + steep, ANGLE_FULL / 4, "complementary about 45°");
    }

    #[test]
    fn atan2_round_trips_sin_cos_within_tolerance() {
        // For a sampling of angles, the vector (cos a, sin a) fed back through atan2 recovers a.
        // The error budget is the sin/cos table truncation plus the atan ratio quantization; well
        // inside a third of a degree, far tighter than any turret cares about.
        const TOL: i32 = 48; // angle-units, roughly a quarter of a degree
        let mut worst = 0;
        let mut a = 0;
        while a < ANGLE_FULL {
            let ang = Angle(a);
            let v = Vec2 { x: cos(ang), y: sin(ang) };
            // Skip degenerate (the table can produce an exact-zero component, still fine here).
            let back = atan2(v.y, v.x);
            let d = shortest_diff(back, ang).abs();
            worst = worst.max(d);
            assert!(d <= TOL, "round-trip at {a} off by {d} (> {TOL})");
            a += 257; // a prime-ish stride to dodge lining up with the LUT spacing
        }
        // Guard against the tolerance silently going slack: the real worst case is well under it.
        assert!(worst <= TOL, "worst round-trip error {worst}");
    }

    // A local 2-vector for the round-trip test (mirrors components::Vec2 without the dependency).
    struct Vec2 {
        x: Fixed,
        y: Fixed,
    }

    // --- rotate_toward -----------------------------------------------------------------------

    #[test]
    fn rotate_toward_snaps_when_within_a_step() {
        assert_eq!(rotate_toward(Angle(100), Angle(110), 50), Angle(110));
        // Exactly one step away → snaps (boundary is inclusive).
        assert_eq!(rotate_toward(Angle(100), Angle(150), 50), Angle(150));
    }

    #[test]
    fn rotate_toward_steps_by_max_when_far() {
        assert_eq!(rotate_toward(Angle(0), Angle(1000), 100), Angle(100));
        // Negative direction: target behind (the short way) → step down.
        assert_eq!(rotate_toward(Angle(1000), Angle(0), 100), Angle(900));
    }

    #[test]
    fn rotate_toward_takes_the_short_way_across_the_seam() {
        // from ≈ +10, target ≈ −10 (i.e. ANGLE_FULL−10): the short way is −20, not +65516.
        let from = Angle(10);
        let target = Angle(ANGLE_FULL - 10);
        // Within a generous step → snaps straight onto target.
        assert_eq!(rotate_toward(from, target, 100), target);
        // Smaller step → moves the short way (decreasing through 0, wrapping).
        assert_eq!(rotate_toward(from, target, 5), Angle((10 - 5) & (ANGLE_FULL - 1)));
        // Mirror: from just below the seam, target just above → short way is +.
        let from2 = Angle(ANGLE_FULL - 6);
        assert_eq!(rotate_toward(from2, Angle(10), 5), Angle((from2.0 + 5) & (ANGLE_FULL - 1)));
    }

    #[test]
    fn rotate_toward_zero_and_negative_step_hold_position() {
        assert_eq!(rotate_toward(Angle(33), Angle(99), 0), Angle(33));
        assert_eq!(rotate_toward(Angle(33), Angle(99), -5), Angle(33));
        // Already there → stays, any step.
        assert_eq!(rotate_toward(Angle(77), Angle(77), 0), Angle(77));
    }

    #[test]
    fn rotate_toward_half_turn_step_always_reaches() {
        assert_eq!(
            rotate_toward(Angle(0), Angle(ANGLE_FULL / 2), ANGLE_FULL / 2),
            Angle(ANGLE_FULL / 2)
        );
    }

    #[test]
    fn rotate_toward_converges_monotonically_and_never_overshoots() {
        let target = Angle(20_000);
        let mut cur = Angle(0);
        let step = 333;
        let mut prev_gap = shortest_diff(target, cur).abs();
        for _ in 0..1000 {
            let next = rotate_toward(cur, target, step);
            let gap = shortest_diff(target, next).abs();
            assert!(gap <= prev_gap, "gap must never grow: {prev_gap} -> {gap}");
            // Until it snaps, each step closes exactly `step` (no overshoot, no stalling).
            if prev_gap > step {
                assert_eq!(prev_gap - gap, step, "closes exactly one step while far");
            }
            cur = next;
            prev_gap = gap;
            if cur == target {
                break;
            }
        }
        assert_eq!(cur, target, "reaches the target");
        // And holds once arrived.
        assert_eq!(rotate_toward(cur, target, step), target);
    }
}

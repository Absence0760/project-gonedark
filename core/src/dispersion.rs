//! Tank aim-time dispersion — the reticle bloom (tank embodiment P5, D55).
//!
//! War Thunder's gun is *not* a hitscan laser: it **blooms** when you move or traverse and
//! **settles** back to a tight cone when you hold still. This module owns that one piece of
//! state — [`Weapon::dispersion`](crate::components::Weapon) — and the three operations on it:
//!
//! 1. [`bloom`] — grow the bloom when the hull moves or the turret traverses (called at the
//!    embodied drive/aim sites in [`Sim::apply`](crate::sim::Sim));
//! 2. [`dispersion_system`] — settle every tank gun's bloom back toward zero each tick;
//! 3. [`scatter_dir`] — perturb a launched shell's direction in proportion to the current bloom.
//!
//! ## Skill-honest (the refinement of plan §5)
//! A **fully settled** gun (`dispersion == 0`) fires **dead-on** the aim with **zero scatter** and
//! draws no RNG — so a mastered shot is *exact*, never robbed by an RNG bullet. Only an unsettled
//! gun (just moved / mid-traverse) scatters, the cone widening with the bloom. Mastery is waiting
//! for the reticle to settle, then committing.
//!
//! ## Embodied-only skill, opt-in by a zero gate (invariants #1, #3, #7)
//! Dispersion is meaningful only for a **ballistic tank gun** (`muzzle_vel > 0`). Both [`bloom`] and
//! [`dispersion_system`] gate on that, so every infantry/hitscan weapon keeps `dispersion == 0` and
//! costs the checksum nothing — exactly the `mag_size`/`turret_speed`/`muzzle_vel` zero-default
//! pattern. Bloom is applied only from the **embodied** drive/aim paths, so an AI tank never gains
//! the aim skill (invariant #3, like ammo/crouch). All `Fixed`/`Angle`/integer — the scatter draws
//! from the sim's reserved deterministic [`Rng`] (an integer draw), so it is bit-identical across
//! every peer (invariant #7).

use crate::components::{Vec2, Weapon};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::rng::Rng;
use crate::systems::approach;
use crate::trig::{self, Angle, ANGLE_FULL};

/// The maximum bloom a tank gun's [`dispersion`](Weapon::dispersion) can reach (the reticle's
/// widest). `1` (Q16.16) is the natural ceiling: [`scatter_dir`] maps the bloom in `[0, MAX]` onto a
/// scatter cone in `[0, SCATTER_MAX_ANGLE]`, so `MAX = ONE` makes that mapping a clean scale by
/// [`SCATTER_MAX_ANGLE`]. Playtest baseline; exact, no float (invariant #1).
pub const DISPERSION_MAX: Fixed = Fixed::ONE;

/// Bloom shed per tick while the tank holds still and steady — the settle rate
/// ([`dispersion_system`]). `1/32` per tick walks the bloom from [`DISPERSION_MAX`] (`1.0`) down to
/// zero in 32 ticks (~0.5 s at the locked 60 Hz): a fully agitated gun settles to pinpoint in about
/// half a second of holding. Playtest baseline; exact ratio keeps it float-free (invariant #1).
pub const DISPERSION_SETTLE: Fixed = Fixed::from_ratio(1, 32);

/// Bloom added per tick the hull is **moving** ([`bloom`] at the embodied `DriveHull` site). `1/16`
/// per tick is strictly larger than [`DISPERSION_SETTLE`] (`1/32`), so a driving tank's reticle
/// **grows** (net `+1/32`/tick) — reaching [`DISPERSION_MAX`] after ~32 ticks of sustained driving —
/// while a stopped one settles. Playtest baseline; exact ratio, no float (invariant #1).
pub const DISPERSION_BLOOM_MOVE: Fixed = Fixed::from_ratio(1, 16);

/// Bloom added per tick the turret is **traversing** ([`bloom`] at the embodied `AimTurret` site).
/// Same magnitude as [`DISPERSION_BLOOM_MOVE`] (`1/16`), so slewing the gun blooms the reticle just
/// as moving the hull does (both outrun the `1/32` settle). Playtest baseline; exact, no float.
pub const DISPERSION_BLOOM_TRAVERSE: Fixed = Fixed::from_ratio(1, 16);

/// Half-angle of the scatter cone at **full bloom** ([`DISPERSION_MAX`]), in angle-units. A
/// [`Vec2`]/[`Angle`] turn is [`ANGLE_FULL`] (`65536`) units, so `ANGLE_FULL/64 = 1024` is a `±5.6°`
/// cone (~11° wide) when the reticle is fully blown — a tank firing on the move sprays, a settled
/// one is exact. A clean power-of-two divisor keeps the scale integer. Playtest baseline; no float.
pub const SCATTER_MAX_ANGLE: i32 = ANGLE_FULL / 64;

/// Does this weapon use the dispersion model? **Only a ballistic tank gun** (`muzzle_vel > 0`). The
/// gate that makes dispersion opt-in: every infantry / hitscan weapon (`muzzle_vel == 0`, the
/// default) is excluded, so its `dispersion` stays `0` and the field is byte-neutral in the
/// per-tick checksum (invariant #7).
#[inline]
pub fn is_tank_gun(w: &Weapon) -> bool {
    w.muzzle_vel > Fixed::ZERO
}

/// Grow entity `i`'s aim bloom by `amount`, clamped to [`DISPERSION_MAX`] — the reticle widening
/// from hull motion or turret traverse (tank embodiment P5, D55). A **no-op** unless `i` carries a
/// ballistic tank gun ([`is_tank_gun`]), so calling it on an infantry mover costs nothing and never
/// perturbs the checksum. Pure fixed-point add + clamp (invariant #1); the caller invokes it only
/// from the **embodied** drive/aim paths, keeping the aim skill first-person-only (invariant #3).
#[inline]
pub fn bloom(world: &mut World, i: usize, amount: Fixed) {
    let w = &mut world.weapon[i];
    if !is_tank_gun(w) {
        return;
    }
    w.dispersion = (w.dispersion + amount).min(DISPERSION_MAX);
}

/// Settle every living tank gun's aim bloom one tick toward zero (the reticle tightening at rest).
/// Runs in [`Sim::step`](crate::sim::Sim::step)'s fixed order each tick; bloom is added separately
/// at the embodied drive/aim sites, so the per-tick net is *bloom − settle* (positive while
/// agitated, negative — toward pinpoint — while held). Gated on [`is_tank_gun`], so a non-tank's
/// `dispersion` never moves (it stays the `0` default ⇒ byte-neutral checksum, invariant #7).
/// Index-ordered, fixed-point only — bit-identical on every peer.
pub fn dispersion_system(world: &mut World) {
    let n = world.capacity();
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        let w = &mut world.weapon[i];
        if !is_tank_gun(w) {
            continue;
        }
        // `approach` toward zero subtracts at most DISPERSION_SETTLE, snapping exactly onto zero
        // without overshoot (and holding at zero once settled — a no-op for an un-bloomed gun).
        w.dispersion = approach(w.dispersion, Fixed::ZERO, DISPERSION_SETTLE);
    }
}

/// Perturb a **unit** aim direction by the current `dispersion`, returning the scattered direction
/// (tank embodiment P5, D55). The skill-honest contract (plan §5):
///
/// - A **fully settled** gun (`dispersion <= 0`) fires **dead-on**: returns `aim_unit` **unchanged**
///   and **draws no RNG** — so a settled shot is byte-identical to the un-dispersed launch and a
///   mastered aim is never robbed by a random bullet.
/// - Otherwise the bloom maps onto a half-cone of `dispersion/DISPERSION_MAX × SCATTER_MAX_ANGLE`
///   angle-units; a single integer draw from the reserved deterministic `rng` picks a uniform
///   offset in `[-half, +half]`, and `aim_unit` is rotated by it (a float-free LUT `cos`/`sin`
///   rotation). Same `dispersion` + same `rng` state ⇒ same offset on every peer (invariant #7).
///
/// `aim_unit` must already be a (roughly) unit vector — callers pass `dir.normalized()`. Float-free
/// throughout (invariant #1): the offset is integer angle-units and the rotation is LUT trig.
pub fn scatter_dir(aim_unit: Vec2, dispersion: Fixed, rng: &mut Rng) -> Vec2 {
    if dispersion <= Fixed::ZERO {
        return aim_unit; // fully settled → dead-on, no RNG draw (the no-scatter byte-identical path)
    }
    let d = dispersion.min(DISPERSION_MAX);
    // Half-cone in angle-units, proportional to the bloom. `d ≤ 1` and SCATTER_MAX_ANGLE is small,
    // so `d * SCATTER_MAX_ANGLE` cannot overflow Q16.16; `>> FRAC_BITS` is the floor to an integer.
    let half = ((d * Fixed::from_int(SCATTER_MAX_ANGLE)).to_bits() >> Fixed::FRAC_BITS).max(0);
    if half == 0 {
        return aim_unit; // bloom too small to deflect a whole angle-unit → dead-on, no draw
    }
    // Uniform integer offset in [-half, +half] from the deterministic sim RNG (integer draw).
    let span = (2 * half + 1) as u32;
    let offset = rng.below(span) as i32 - half;
    let a = Angle(offset);
    let (c, s) = (trig::cos(a), trig::sin(a));
    // Standard 2D rotation of the aim by `offset` (LUT cos/sin — no transcendental, invariant #1).
    Vec2::new(
        aim_unit.x * c - aim_unit.y * s,
        aim_unit.x * s + aim_unit.y * c,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Faction, Health, InputSource};
    use crate::fixed::Fixed;

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    /// Spawn a unit carrying a ballistic tank gun (`muzzle_vel > 0`), returning its slot index.
    fn spawn_tank(world: &mut World) -> usize {
        let e = world.spawn();
        let i = e.index as usize;
        world.faction[i] = Faction::Player;
        world.health[i] = Health::full(fx(1000));
        world.input_source[i] = InputSource::Embodied;
        world.weapon[i] = Weapon {
            range: fx(30),
            damage: fx(50),
            turret_speed: 200,
            muzzle_vel: fx(2),
            ..Weapon::default()
        };
        i
    }

    #[test]
    fn bloom_grows_and_clamps_at_max() {
        let mut world = World::new();
        let i = spawn_tank(&mut world);
        assert_eq!(world.weapon[i].dispersion, Fixed::ZERO, "starts settled");
        bloom(&mut world, i, DISPERSION_BLOOM_MOVE);
        assert_eq!(world.weapon[i].dispersion, DISPERSION_BLOOM_MOVE, "one move bloom");
        bloom(&mut world, i, DISPERSION_BLOOM_TRAVERSE);
        assert_eq!(
            world.weapon[i].dispersion,
            DISPERSION_BLOOM_MOVE + DISPERSION_BLOOM_TRAVERSE,
            "move + traverse stack",
        );
        // Pile on far past the ceiling: clamps to MAX, never beyond.
        for _ in 0..100 {
            bloom(&mut world, i, DISPERSION_BLOOM_MOVE);
        }
        assert_eq!(world.weapon[i].dispersion, DISPERSION_MAX, "bloom clamps at the cap");
    }

    #[test]
    fn bloom_is_a_no_op_for_an_infantry_weapon() {
        // A hitscan weapon (muzzle_vel == 0) is gated out: its dispersion never moves, so the field
        // stays the byte-neutral zero default (invariant #7).
        let mut world = World::new();
        let e = world.spawn();
        let i = e.index as usize;
        world.weapon[i] = Weapon {
            range: fx(10),
            damage: fx(25),
            muzzle_vel: Fixed::ZERO, // infantry / hitscan
            ..Weapon::default()
        };
        bloom(&mut world, i, DISPERSION_BLOOM_MOVE);
        assert_eq!(world.weapon[i].dispersion, Fixed::ZERO, "infantry never blooms");
    }

    #[test]
    fn dispersion_system_settles_toward_zero_and_clamps() {
        let mut world = World::new();
        let i = spawn_tank(&mut world);
        world.weapon[i].dispersion = DISPERSION_MAX;
        // Each tick sheds exactly one settle step until it reaches zero, then holds.
        let mut prev = DISPERSION_MAX;
        for _ in 0..40 {
            dispersion_system(&mut world);
            let now = world.weapon[i].dispersion;
            assert!(now <= prev, "bloom never grows while holding");
            prev = now;
        }
        assert_eq!(world.weapon[i].dispersion, Fixed::ZERO, "settles fully to pinpoint");
        // Below one step → clamps to zero, never negative.
        world.weapon[i].dispersion = DISPERSION_SETTLE - Fixed::from_ratio(1, 256);
        dispersion_system(&mut world);
        assert_eq!(world.weapon[i].dispersion, Fixed::ZERO, "a sub-step remainder snaps to zero");
    }

    #[test]
    fn dispersion_system_ignores_infantry_weapons() {
        // A muzzle_vel == 0 weapon with a (synthetic) non-zero dispersion is left untouched: the
        // gate keeps the system byte-neutral for every non-tank slot.
        let mut world = World::new();
        let e = world.spawn();
        let i = e.index as usize;
        world.weapon[i].muzzle_vel = Fixed::ZERO;
        world.weapon[i].dispersion = DISPERSION_MAX; // shouldn't happen in practice, but must not move
        dispersion_system(&mut world);
        assert_eq!(world.weapon[i].dispersion, DISPERSION_MAX, "infantry dispersion is never settled");
    }

    #[test]
    fn settled_gun_fires_dead_on_with_no_rng_draw() {
        // The skill-honest core: a fully settled gun returns the aim UNCHANGED and advances the RNG
        // by nothing (so a mastered shot is exact and byte-identical to an un-dispersed launch).
        let aim = Vec2::new(Fixed::ONE, Fixed::ZERO);
        let mut rng = Rng::new(0xD15);
        let before = rng.checksum_state();
        let out = scatter_dir(aim, Fixed::ZERO, &mut rng);
        assert_eq!(out, aim, "settled gun fires dead-on the aim");
        assert_eq!(rng.checksum_state(), before, "a settled shot draws no RNG (no perturbation)");
    }

    #[test]
    fn moving_gun_scatters_within_the_bloom_cone_and_is_deterministic() {
        // A blown reticle deflects the shot off the aim, bounded by the cone for that bloom, and two
        // RNGs on the same seed produce the IDENTICAL scattered direction (lockstep, invariant #7).
        let aim = Vec2::new(Fixed::ONE, Fixed::ZERO);
        let mut a = Rng::new(0x5CA7);
        let mut b = Rng::new(0x5CA7);
        let mut deflected_at_least_once = false;
        for _ in 0..64 {
            let da = scatter_dir(aim, DISPERSION_MAX, &mut a);
            let db = scatter_dir(aim, DISPERSION_MAX, &mut b);
            assert_eq!(da, db, "same seed → bit-identical scatter (lockstep)");
            if da != aim {
                deflected_at_least_once = true;
                // Within the cone: at the cap the |y| component cannot exceed sin(SCATTER_MAX_ANGLE)
                // (the aim is +X, so the rotated y is sin(offset), |offset| ≤ SCATTER_MAX_ANGLE).
                let bound = trig::sin(Angle(SCATTER_MAX_ANGLE));
                assert!(da.y.abs() <= bound, "scatter stays inside the bloom cone");
            }
        }
        assert!(deflected_at_least_once, "a fully-blown gun actually scatters across many shots");
    }

    #[test]
    fn wider_bloom_can_scatter_wider_than_a_tighter_one() {
        // The cone scales with the bloom: the maximum |y| deflection at full bloom strictly exceeds
        // that at a small bloom (the bound is proportional to dispersion).
        let small = ((Fixed::from_ratio(1, 8) * Fixed::from_int(SCATTER_MAX_ANGLE)).to_bits()
            >> Fixed::FRAC_BITS)
            .max(0);
        let full = ((DISPERSION_MAX * Fixed::from_int(SCATTER_MAX_ANGLE)).to_bits()
            >> Fixed::FRAC_BITS)
            .max(0);
        assert!(full > small, "a wider bloom permits a wider scatter cone");
    }
}

//! Sim systems — pure functions over component spans, iterated in stable index order.
//!
//! Movement is the literal executor (invariant #3, D3): a unit holds its last `Order` and
//! does exactly that — step toward its target at a fixed speed, no autonomy. The full
//! order/stance vocabulary lives in [`orders::order_system`](crate::orders); this module owns
//! only the shared stepping primitive both it (and any future mover) call.
//!
//! Pathing uses a real deterministic [`FlowField`](crate::flow_field): a unit steps along the
//! sampled downhill direction toward its target. Fields come from a per-tick
//! [`FlowFieldCache`](crate::flow_field::FlowFieldCache) — units sharing a goal share one build,
//! which is bit-identical to each building its own (a field is a pure function of its goal) but
//! turns a 200-unit shared push from ~200 builds into a handful (the measured 60 Hz bottleneck;
//! `docs/phase-3-plan.md` §"Workstream A"). Phase 1 had no obstacles, so the field points at the
//! goal; the structure generalises to Phase 2 terrain.

use crate::components::{EntityKind, InputSource, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::flow_field::FlowFieldCache;
use crate::trig;

/// Base move speed in world units per tick (1/8). Tune via data later.
pub const MOVE_SPEED: Fixed = Fixed::from_ratio(1, 8);

/// Maximum hull turn rate, angle-units per tick (tank embodiment P2, D55). A full turn is
/// [`trig::ANGLE_FULL`](crate::trig::ANGLE_FULL) `= 65536` units, so `256/tick` at the locked 60 Hz
/// is `256·60/65536` of a turn per second ≈ a 84°/s chassis traverse — a deliberately heavy,
/// turn-then-drive feel (the hull cannot snap to the stick). Playtest baseline, not final feel:
/// dial against the embodied tank's handling once P7/P8 make it visible.
pub const HULL_TURN_RATE: i32 = 256;

/// Hull acceleration/braking clamp, `Fixed` of speed change per tick (tank embodiment P2, D55).
/// `1/256` per tick ramps `hull_speed` from rest to the full [`MOVE_SPEED`] (`1/8 = 32/256`) over
/// 32 ticks (about half a second at 60 Hz) and brakes it back symmetrically — the inertia that
/// makes the tank feel weighty instead of teleporting to stick speed. Playtest baseline; exact
/// ratio keeps it float-free (invariant #1).
pub const HULL_ACCEL: Fixed = Fixed::from_ratio(1, 256);

/// Embodied move speed while **crouched** (1/16 = half [`MOVE_SPEED`]). The mobility cost of the
/// marksman stance — crouching halves your walking speed in exchange for the tighter, longer-
/// ranged shot (`combat`). Exact ratio keeps it float-free (invariant #1). Applied only to the
/// embodied `Command::Locomote` path (AI units never crouch — invariant #3).
pub const CROUCH_MOVE_SPEED: Fixed = Fixed::from_ratio(1, 16);

/// Squared arrival epsilon: snap to the target when closer than this (1/256 units²).
pub const ARRIVE_EPS_SQ: Fixed = Fixed::from_ratio(1, 256);

/// Step a single unit toward `target` via the flow field at an explicit `speed` (world units
/// per tick). The field is fetched from `cache` (built once per distinct goal per tick), so the
/// sampled direction is bit-identical to building a fresh field here. Returns `true` once it has
/// arrived (within [`ARRIVE_EPS_SQ`]), snapping it onto the target and zeroing velocity. The one
/// movement implementation `orders::order_system` builds on (invariant #3 — the unit only
/// follows the field, it does not strategize). A `speed` of zero pins the unit in place (e.g.
/// fully suppressed) without completing its order — and without forcing a field build.
pub fn step_toward_speed(
    world: &mut World,
    cache: &mut FlowFieldCache,
    i: usize,
    target: Vec2,
    speed: Fixed,
) -> bool {
    let to = target - world.pos[i];
    if to.len_sq() <= ARRIVE_EPS_SQ {
        world.pos[i] = target;
        world.vel[i] = Vec2::ZERO;
        true
    } else if speed == Fixed::ZERO {
        // Pinned: hold position, but the order is not yet complete.
        world.vel[i] = Vec2::ZERO;
        false
    } else {
        let dir = cache.get(target).sample(world.pos[i]);
        let step = dir.scale(speed);
        world.vel[i] = step;
        world.pos[i] = world.pos[i] + step;
        false
    }
}

/// Step a single unit toward `target` at the base [`MOVE_SPEED`].
#[inline]
pub fn step_toward(world: &mut World, cache: &mut FlowFieldCache, i: usize, target: Vec2) -> bool {
    step_toward_speed(world, cache, i, target, MOVE_SPEED)
}

/// Step a single unit along an explicit `dir` at `speed` (world units per tick) — the
/// flow-field-free mover for the **embodied** avatar, whose heading comes straight from live
/// player input, not a goal ([`orders::order_system`](crate::orders) skips embodied units). `dir`
/// is the desired heading already quantized to `Fixed` at the host boundary (invariant #1, same
/// rule as the [`Command::Fire`](crate::sim::Command::Fire) aim); its magnitude is the analog
/// deflection (`≤ 1` for a clamped stick, so partial deflection moves proportionally slower).
/// Unlike [`step_toward_speed`] there is no arrival test — locomotion is continuous and re-issued
/// every tick — so this just sets velocity and integrates position. A zero `dir` parks the unit.
pub fn step_along(world: &mut World, i: usize, dir: Vec2, speed: Fixed) {
    let step = dir.scale(speed);
    world.vel[i] = step;
    world.pos[i] = world.pos[i] + step;
}

/// Move `cur` toward `target` by at most `max_step`, snapping exactly onto `target` once within a
/// step — a scalar rate-limiter with **no overshoot** (the `Fixed` analogue of
/// [`trig::rotate_toward`]). `max_step` is clamped non-negative. Used by `DriveHull` to
/// accelerate/brake `hull_speed` toward the stick target (tank inertia). Pure fixed-point.
#[inline]
pub fn approach(cur: Fixed, target: Fixed, max_step: Fixed) -> Fixed {
    let step = max_step.max(Fixed::ZERO);
    let delta = target - cur;
    if delta.abs() <= step {
        target
    } else if delta > Fixed::ZERO {
        cur + step
    } else {
        cur - step
    }
}

/// Drive an embodied tank's chassis one tick along `dir` (the move-stick), with turn-rate-limited
/// steering + inertia — the vehicle counterpart to [`step_along`]'s instant infantry strafe (tank
/// embodiment P2, D55). When `dir` is non-zero the hull turns toward `atan2(dir)` by at most
/// [`HULL_TURN_RATE`]; `hull_speed` then accelerates/brakes toward `MOVE_SPEED · |dir|` (clamped to
/// `1`) by [`HULL_ACCEL`] (a near-zero / released stick brakes to rest), and the unit advances
/// along its **hull heading** (`cos`/`sin`) at the resulting speed. The caller gates this on
/// `alive && Embodied`; AI hulls are turned cosmetically by [`heading_system`] instead.
pub fn drive_hull(world: &mut World, i: usize, dir: Vec2) {
    // Steer only on a live stick — atan2(0,0) is 0 by convention, so braking with a released
    // stick must NOT snap the hull toward +X. Hold the heading and let speed bleed off.
    let mut target_speed = Fixed::ZERO;
    if dir != Vec2::ZERO {
        let bearing = trig::atan2(dir.y, dir.x);
        world.hull_heading[i] = trig::rotate_toward(world.hull_heading[i], bearing, HULL_TURN_RATE);
        // Analog deflection scales the target speed; clamp to a unit stick so an over-long vector
        // can't exceed MOVE_SPEED.
        let deflection = dir.len().min(Fixed::ONE);
        target_speed = MOVE_SPEED * deflection;
    }
    world.hull_speed[i] = approach(world.hull_speed[i], target_speed, HULL_ACCEL);
    // Advance along the hull-heading unit vector at the inertial speed (reuses step_along, so vel
    // is set consistently with the infantry mover).
    let h = world.hull_heading[i];
    let fwd = Vec2::new(trig::cos(h), trig::sin(h));
    step_along(world, i, fwd, world.hull_speed[i]);
}

/// Cosmetic AI heading slew (tank embodiment P2, D55): point every living, **non-embodied** unit's
/// hull along its current velocity (rate-limited by [`HULL_TURN_RATE`]) and slew its turret toward
/// the hull at the weapon's `turret_speed`. This keeps an AI vehicle visually coherent without
/// granting it autonomous aim — invariant #3: the turret merely follows the chassis (`turret_speed
/// == 0`, the infantry default, is a no-op so non-tanks are untouched). Embodied units are skipped;
/// the player drives them through `DriveHull`/`AimTurret`. Buildings have no hull and are skipped.
/// Called in [`Sim::step`](crate::sim::Sim::step) AFTER `order_system` has set this tick's velocity.
pub fn heading_system(world: &mut World) {
    let n = world.capacity();
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        if world.kind[i] == EntityKind::Building {
            continue;
        }
        if world.input_source[i] == InputSource::Embodied {
            continue;
        }
        // Turn the hull toward the way the unit is actually moving (a parked unit holds heading).
        if world.vel[i] != Vec2::ZERO {
            let bearing = trig::atan2(world.vel[i].y, world.vel[i].x);
            world.hull_heading[i] =
                trig::rotate_toward(world.hull_heading[i], bearing, HULL_TURN_RATE);
        }
        // Turret tracks the hull at its slew rate (0 for infantry → no-op, stays Angle(0)).
        let step = world.weapon[i].turret_speed as i32;
        world.turret_yaw[i] = trig::rotate_toward(world.turret_yaw[i], world.hull_heading[i], step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_unit() -> (World, usize) {
        let mut w = World::new();
        let e = w.spawn();
        (w, e.index as usize)
    }

    #[test]
    fn step_along_advances_by_dir_times_speed() {
        let (mut w, i) = world_with_unit();
        // +x unit heading at the base move speed: one tick advances exactly MOVE_SPEED in x.
        step_along(&mut w, i, Vec2::new(Fixed::ONE, Fixed::ZERO), MOVE_SPEED);
        assert_eq!(w.pos[i], Vec2::new(MOVE_SPEED, Fixed::ZERO));
        assert_eq!(w.vel[i], Vec2::new(MOVE_SPEED, Fixed::ZERO));
    }

    #[test]
    fn step_along_accumulates_across_ticks() {
        let (mut w, i) = world_with_unit();
        let dir = Vec2::new(Fixed::ZERO, Fixed::ONE);
        step_along(&mut w, i, dir, MOVE_SPEED);
        step_along(&mut w, i, dir, MOVE_SPEED);
        assert_eq!(w.pos[i], Vec2::new(Fixed::ZERO, MOVE_SPEED * Fixed::from_int(2)));
    }

    #[test]
    fn half_deflection_walks_at_half_speed() {
        // Analog magnitude carries through: a half-pushed stick covers half the ground.
        let (mut w, i) = world_with_unit();
        let half = Fixed::from_ratio(1, 2);
        step_along(&mut w, i, Vec2::new(half, Fixed::ZERO), MOVE_SPEED);
        assert_eq!(w.pos[i].x, MOVE_SPEED * half);
    }

    #[test]
    fn zero_dir_parks_the_unit() {
        let (mut w, i) = world_with_unit();
        w.pos[i] = Vec2::new(Fixed::from_int(3), Fixed::from_int(4));
        w.vel[i] = Vec2::new(MOVE_SPEED, MOVE_SPEED);
        step_along(&mut w, i, Vec2::ZERO, MOVE_SPEED);
        assert_eq!(w.pos[i], Vec2::new(Fixed::from_int(3), Fixed::from_int(4)));
        assert_eq!(w.vel[i], Vec2::ZERO);
    }

    // --- tank embodiment P2 (D55): approach / drive_hull / heading_system --------------------

    #[test]
    fn approach_clamps_steps_and_snaps_without_overshoot() {
        let step = Fixed::from_ratio(1, 4);
        // Far below target: rise by exactly one step.
        assert_eq!(approach(Fixed::ZERO, Fixed::ONE, step), step);
        // Within a step of the target: snap exactly (no overshoot).
        assert_eq!(approach(Fixed::from_ratio(7, 8), Fixed::ONE, step), Fixed::ONE);
        // Above target (braking): fall by one step toward it.
        assert_eq!(approach(Fixed::ONE, Fixed::ZERO, step), Fixed::ONE - step);
        // A negative max_step is clamped to zero → holds.
        assert_eq!(approach(Fixed::HALF, Fixed::ONE, Fixed::from_int(-3)), Fixed::HALF);
    }

    #[test]
    fn drive_hull_turns_toward_the_stick_and_accelerates_from_rest() {
        // Aim due +Y from a hull facing +X: the hull must rotate toward a quarter turn, by no more
        // than HULL_TURN_RATE this tick, and hull_speed must rise from 0 by exactly HULL_ACCEL.
        let (mut w, i) = world_with_unit();
        let north = Vec2::new(Fixed::ZERO, Fixed::ONE);
        assert_eq!(w.hull_heading[i], crate::trig::Angle(0));
        drive_hull(&mut w, i, north);
        assert_eq!(
            w.hull_heading[i],
            crate::trig::Angle(HULL_TURN_RATE),
            "hull turns toward +Y by exactly one step",
        );
        assert_eq!(w.hull_speed[i], HULL_ACCEL, "accelerates from rest by one step");
        // Position advances along the (new) hull heading, not straight at the stick.
        assert_ne!(w.pos[i], Vec2::ZERO, "the tank moved");
    }

    #[test]
    fn drive_hull_caps_speed_at_move_speed_and_advances_along_heading() {
        // Hold the stick straight ahead (+X, the initial heading) long enough to reach the speed
        // cap; the hull never turns (already aligned) and speed plateaus at MOVE_SPEED.
        let (mut w, i) = world_with_unit();
        let east = Vec2::new(Fixed::ONE, Fixed::ZERO);
        for _ in 0..200 {
            drive_hull(&mut w, i, east);
        }
        assert_eq!(w.hull_heading[i], crate::trig::Angle(0), "stays aligned to +X");
        assert_eq!(w.hull_speed[i], MOVE_SPEED, "speed saturates at the cap, no overshoot");
        // Moving straight +X, y stays put and x has advanced well past one tick of travel.
        assert_eq!(w.pos[i].y, Fixed::ZERO);
        assert!(w.pos[i].x > MOVE_SPEED, "covered ground along the hull heading");
    }

    #[test]
    fn drive_hull_partial_deflection_caps_speed_at_scaled_move_speed() {
        // A half-pushed move-stick aligned with the hull steers nothing (already on-bearing) but
        // scales the steady-state speed: target = MOVE_SPEED · |dir| = MOVE_SPEED · 1/2. The hull
        // heading stays at Angle(0) throughout, so the analog deflection is the only variable.
        let (mut w, i) = world_with_unit();
        let half = Fixed::from_ratio(1, 2);
        let stick = Vec2::new(half, Fixed::ZERO); // half deflection, due +X (the initial heading)
        for _ in 0..200 {
            drive_hull(&mut w, i, stick);
        }
        assert_eq!(w.hull_heading[i], crate::trig::Angle(0), "stays aligned to +X");
        assert_eq!(
            w.hull_speed[i],
            MOVE_SPEED * half,
            "steady-state speed is the deflection-scaled cap, not the full MOVE_SPEED",
        );
    }

    #[test]
    fn drive_hull_release_brakes_to_a_full_stop() {
        // Spin up, then release the stick (zero dir): speed bleeds off by HULL_ACCEL/tick to 0 and
        // the heading is held (a released stick must not snap the hull toward +X).
        let (mut w, i) = world_with_unit();
        let east = Vec2::new(Fixed::ONE, Fixed::ZERO);
        for _ in 0..50 {
            drive_hull(&mut w, i, east);
        }
        assert_eq!(w.hull_speed[i], MOVE_SPEED);
        let held = w.hull_heading[i];
        for _ in 0..200 {
            drive_hull(&mut w, i, Vec2::ZERO);
        }
        assert_eq!(w.hull_speed[i], Fixed::ZERO, "released stick brakes to a halt");
        assert_eq!(w.hull_heading[i], held, "heading held while braking");
        assert_eq!(w.vel[i], Vec2::ZERO, "stopped tank has zero velocity");
    }

    #[test]
    fn heading_system_turns_an_ai_hull_toward_its_velocity() {
        // An order-driven unit moving +Y: heading_system rotates its hull toward a quarter turn by
        // one HULL_TURN_RATE step per call (the cosmetic AI slew, invariant #3).
        let (mut w, i) = world_with_unit();
        w.vel[i] = Vec2::new(Fixed::ZERO, Fixed::ONE);
        heading_system(&mut w);
        assert_eq!(w.hull_heading[i], crate::trig::Angle(HULL_TURN_RATE));
        // Repeated calls converge onto the bearing and then hold there.
        for _ in 0..ANGLE_FULL_QUARTER_STEPS {
            heading_system(&mut w);
        }
        assert_eq!(
            w.hull_heading[i],
            crate::trig::Angle(crate::trig::ANGLE_FULL / 4),
            "converges onto +Y and holds",
        );
    }

    /// Enough `heading_system` ticks to slew a hull a full quarter turn at [`HULL_TURN_RATE`]
    /// (plus slack) — used to drive the AI-slew convergence assertion above.
    const ANGLE_FULL_QUARTER_STEPS: usize =
        (crate::trig::ANGLE_FULL / 4 / HULL_TURN_RATE) as usize + 2;

    #[test]
    fn heading_system_leaves_an_embodied_unit_untouched() {
        // The player drives an embodied tank via commands; the AI slew must skip it entirely, even
        // with a non-zero velocity present.
        let (mut w, i) = world_with_unit();
        w.input_source[i] = InputSource::Embodied;
        w.vel[i] = Vec2::new(Fixed::ZERO, Fixed::ONE);
        w.hull_heading[i] = crate::trig::Angle(12_345);
        w.turret_yaw[i] = crate::trig::Angle(6_789);
        heading_system(&mut w);
        assert_eq!(w.hull_heading[i], crate::trig::Angle(12_345), "hull untouched");
        assert_eq!(w.turret_yaw[i], crate::trig::Angle(6_789), "turret untouched");
    }

    #[test]
    fn heading_system_turret_tracks_hull_at_turret_speed() {
        // With a real turret_speed the AI turret slews toward the hull heading by that many units
        // per tick; with the infantry default (0) it is pinned to Angle(0).
        let (mut w, i) = world_with_unit();
        w.hull_heading[i] = crate::trig::Angle(1_000);
        w.weapon[i].turret_speed = 100;
        heading_system(&mut w); // vel is zero, so the hull holds; only the turret slews
        assert_eq!(w.hull_heading[i], crate::trig::Angle(1_000));
        assert_eq!(w.turret_yaw[i], crate::trig::Angle(100), "turret stepped toward the hull");

        // turret_speed 0 (infantry) never moves the turret.
        let (mut w2, j) = world_with_unit();
        w2.hull_heading[j] = crate::trig::Angle(1_000);
        heading_system(&mut w2);
        assert_eq!(w2.turret_yaw[j], crate::trig::Angle(0), "fixed mount stays put");
    }
}

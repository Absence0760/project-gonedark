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

/// Collision radius of a building footprint, world units (metres). The camp greybox is ~3.5 × 3.0 m
/// (`tools/models/gen_models.py` `build_camp_hq`); `7/4 = 1.75 m` is half its long side, so the
/// circular footprint hugs the walls — you stop *at* the structure, not a step short or a step
/// inside. Exact ratio keeps it float-free (invariant #1).
pub const BUILDING_RADIUS: Fixed = Fixed::from_ratio(7, 4);

/// Body radius of a mover (unit/avatar) for building collision, world units. A trooper is ~0.45 m
/// wide (`build_trooper`), so `1/4 = 0.25 m` is a touch over its half-width — enough that the body
/// keeps its skin out of the wall rather than clipping it. Added to [`BUILDING_RADIUS`] for the
/// push-out distance. Exact ratio keeps it float-free (invariant #1).
pub const UNIT_RADIUS: Fixed = Fixed::from_ratio(1, 4);

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

/// Resolve mover-vs-building overlap by pushing any non-building entity out of every building's
/// circular footprint — the "you can't walk through a building" rule. Run AFTER all movement for the
/// tick (the embodied avatar's `step_along`/`drive_hull` in the command phase, and AI units in
/// [`order_system`](crate::orders::order_system)), so it corrects the final positions before the
/// snapshot. Applies to the embodied player and AI units alike (invariant #3 untouched — this is
/// physics, not a decision: a unit that was *ordered* to walk somewhere still walks, it just can't
/// occupy a wall).
///
/// For each alive building (centre `c`, radius [`BUILDING_RADIUS`]) and each alive non-building
/// entity (centre `p`, radius [`UNIT_RADIUS`]): if `|p − c| < BUILDING_RADIUS + UNIT_RADIUS` the
/// entity sits inside the footprint, so it is moved radially out onto the boundary circle at exactly
/// that sum distance. A unit sitting *exactly* on the centre (zero delta — no defined push
/// direction) is ejected along `+X` deterministically, so every peer resolves the degenerate case
/// identically. Velocity is left untouched (next tick's input/order re-drives it); the position
/// correction alone keeps the body out of the structure.
///
/// All-integer fixed-point (`len_sq`/`normalized` use the deterministic fixed sqrt — invariant #1),
/// iterated in stable index order, so it is bit-identical across the lockstep matrix (invariant #7).
/// Buildings never push each other (they are static and placed non-overlapping). Idempotent: a
/// second pass on an already-resolved world is a no-op (the entity sits *on* the boundary, where the
/// strict `<` test no longer fires).
pub fn resolve_building_collisions(world: &mut World) {
    let n = world.capacity();
    let min_dist = BUILDING_RADIUS + UNIT_RADIUS;
    let min_sq = min_dist * min_dist;
    for b in 0..n {
        if !world.is_index_alive(b) || world.kind[b] != EntityKind::Building {
            continue;
        }
        let center = world.pos[b];
        for e in 0..n {
            if e == b || !world.is_index_alive(e) || world.kind[e] == EntityKind::Building {
                continue;
            }
            let delta = world.pos[e] - center;
            if delta.len_sq() >= min_sq {
                continue; // outside (or exactly on) the footprint — nothing to correct
            }
            // Inside: snap the entity onto the boundary along the outward normal. A zero delta has
            // no direction, so eject along +X (a fixed, peer-identical choice).
            let dir = delta.normalized();
            let out = if dir == Vec2::ZERO {
                Vec2::new(min_dist, Fixed::ZERO)
            } else {
                dir.scale(min_dist)
            };
            world.pos[e] = center + out;
        }
    }
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

    // ---- building collision (a building is solid — you can't walk through it) ----

    /// A building at `(bx, by)` plus a unit at `(ux, uy)`; returns `(world, building_idx, unit_idx)`.
    fn world_with_building_and_unit(
        bx: Fixed,
        by: Fixed,
        ux: Fixed,
        uy: Fixed,
    ) -> (World, usize, usize) {
        let mut w = World::new();
        let b = w.spawn().index as usize;
        let u = w.spawn().index as usize;
        w.kind[b] = EntityKind::Building;
        w.pos[b] = Vec2::new(bx, by);
        w.pos[u] = Vec2::new(ux, uy);
        (w, b, u)
    }

    /// The boundary distance the resolver snaps an overlapping unit to (exactly 2.0 m: 1.75 + 0.25).
    const MIN_DIST: Fixed = Fixed::from_int(2);

    #[test]
    fn building_radii_sum_to_a_clean_two_metres() {
        // The push-out distance is BUILDING_RADIUS + UNIT_RADIUS; the tests below assert exact
        // positions, which rely on that sum being exactly representable.
        assert_eq!(BUILDING_RADIUS + UNIT_RADIUS, MIN_DIST);
    }

    #[test]
    fn building_pushes_an_overlapping_unit_out_to_its_boundary() {
        // Unit half a metre off the centre along +X — deep inside the footprint. It is ejected
        // straight out along +X onto the boundary, exactly MIN_DIST away (the (1,0) normalize is
        // exact in fixed-point, so no rounding drift).
        let (mut w, _b, u) =
            world_with_building_and_unit(Fixed::ZERO, Fixed::ZERO, Fixed::HALF, Fixed::ZERO);
        resolve_building_collisions(&mut w);
        assert_eq!(w.pos[u], Vec2::new(MIN_DIST, Fixed::ZERO));
    }

    #[test]
    fn building_off_origin_pushes_relative_to_its_centre() {
        // The push is radial about the building's actual centre, not the world origin.
        let c = Vec2::new(Fixed::from_int(10), Fixed::from_int(-4));
        let (mut w, _b, u) = world_with_building_and_unit(
            c.x,
            c.y,
            c.x + Fixed::HALF, // just east of the centre, inside the footprint
            c.y,
        );
        resolve_building_collisions(&mut w);
        assert_eq!(w.pos[u], Vec2::new(c.x + MIN_DIST, c.y));
    }

    #[test]
    fn unit_clear_of_the_footprint_is_untouched() {
        // Distance 3.0 m > MIN_DIST (2.0) → no overlap, position unchanged.
        let start = Vec2::new(Fixed::from_int(3), Fixed::ZERO);
        let (mut w, _b, u) =
            world_with_building_and_unit(Fixed::ZERO, Fixed::ZERO, start.x, start.y);
        resolve_building_collisions(&mut w);
        assert_eq!(w.pos[u], start, "outside the footprint, the unit does not move");
    }

    #[test]
    fn unit_exactly_on_the_centre_is_ejected_along_plus_x() {
        // Zero delta has no defined outward direction; the resolver ejects along +X so every peer
        // resolves this degenerate case identically (determinism).
        let (mut w, _b, u) =
            world_with_building_and_unit(Fixed::ZERO, Fixed::ZERO, Fixed::ZERO, Fixed::ZERO);
        resolve_building_collisions(&mut w);
        assert_eq!(w.pos[u], Vec2::new(MIN_DIST, Fixed::ZERO));
    }

    #[test]
    fn diagonal_overlap_is_pushed_to_the_boundary_along_the_normal() {
        // A unit inside on a diagonal is pushed out along that same diagonal to ~MIN_DIST away.
        // The normalize→scale round-trip rounds in fixed-point, so this checks within a small eps.
        let (mut w, _b, u) =
            world_with_building_and_unit(Fixed::ZERO, Fixed::ZERO, Fixed::ONE, Fixed::ONE);
        resolve_building_collisions(&mut w);
        let p = w.pos[u];
        assert_eq!(p.x, p.y, "stayed on the (1,1) diagonal it was pushed along");
        let drift = (p.len() - MIN_DIST).abs();
        assert!(drift <= Fixed::from_ratio(1, 64), "on the boundary, got len {:?}", p.len());
        // The fixed-point sqrt truncates, so normalize overshoots slightly: the unit lands ON or
        // just OUTSIDE the boundary, never inside — which is what makes the push idempotent (a
        // second pass sees `len_sq >= min_sq` and does nothing). Prove it rather than tolerate it.
        let min_sq = MIN_DIST * MIN_DIST;
        assert!(p.len_sq() >= min_sq, "must not land inside the footprint");
    }

    #[test]
    fn resolve_is_idempotent() {
        // A second pass on an already-resolved world is a no-op: the unit sits exactly on the
        // boundary, where the strict inside-test no longer fires.
        let (mut w, _b, u) =
            world_with_building_and_unit(Fixed::ZERO, Fixed::ZERO, Fixed::HALF, Fixed::ZERO);
        resolve_building_collisions(&mut w);
        let once = w.pos[u];
        resolve_building_collisions(&mut w);
        assert_eq!(w.pos[u], once, "settled position is stable under re-resolution");
    }

    #[test]
    fn the_embodied_avatar_collides_too() {
        // Collision is physics, not a decision (invariant #3): it applies to the embodied player
        // exactly as to AI units — the input source is irrelevant.
        let (mut w, _b, u) =
            world_with_building_and_unit(Fixed::ZERO, Fixed::ZERO, Fixed::HALF, Fixed::ZERO);
        w.input_source[u] = InputSource::Embodied;
        resolve_building_collisions(&mut w);
        assert_eq!(w.pos[u], Vec2::new(MIN_DIST, Fixed::ZERO));
    }

    #[test]
    fn buildings_do_not_push_each_other() {
        // Two overlapping buildings (placement should never do this, but the rule must hold): both
        // are static, so neither is moved by the resolver.
        let mut w = World::new();
        let a = w.spawn().index as usize;
        let b = w.spawn().index as usize;
        w.kind[a] = EntityKind::Building;
        w.kind[b] = EntityKind::Building;
        w.pos[a] = Vec2::new(Fixed::ZERO, Fixed::ZERO);
        w.pos[b] = Vec2::new(Fixed::HALF, Fixed::ZERO);
        resolve_building_collisions(&mut w);
        assert_eq!(w.pos[a], Vec2::new(Fixed::ZERO, Fixed::ZERO));
        assert_eq!(w.pos[b], Vec2::new(Fixed::HALF, Fixed::ZERO));
    }
}

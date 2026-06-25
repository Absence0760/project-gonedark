//! The order/stance executor — the literal-executor unit AI (invariant #3, D3/D23).
//!
//! This is where the **depth lives in the order vocabulary, not the AI brain** (game-design
//! §8): a unit holds its last [`Order`](crate::components::Order) + [`Stance`] and does
//! exactly that, every tick, forever, with no autonomy. `order_system` is the sole mover (it
//! built on the Phase 1 movement, now folded into [`systems::step_toward`]) and handles the
//! full Phase 2 vocabulary:
//!
//! - `Idle` / `HoldPosition` — stand still (zero velocity).
//! - `MoveTo` — flow-field walk to a point, then go `Idle` (Phase 1 behavior, preserved exactly).
//! - `AttackMove` — flow-field walk to a point, then `HoldPosition`; combat fires en route.
//! - `Patrol { a, b, toward_b }` — walk to the current leg's end, then flip `toward_b` and
//!   head back — forever.
//! - `FallBack(rally)` — flow-field walk to the rally point; TERMINAL (the unit holds there,
//!   keeping the order, so the retreat trigger below cannot re-fire it).
//!
//! **Retreat trigger (D23):** a unit whose `retreat_below` fraction is set and whose health
//! fraction drops under it has its order *replaced* with `FallBack(rally)`. This is the player
//! pre-programming a reaction ("fall back at 30% HP"), NOT the unit deciding for itself — the
//! literal-executor rule still holds. **Suppression (from `combat`) slows movement.** Embodied
//! units are skipped — their motion comes from live player input (invariant #5).
//!
//! Determinism: fixed-point only, stable index iteration, no floats / transcendentals / hash
//! iteration (the determinism guard greps this file). `MoveTo`/`Idle` for an unsuppressed unit
//! is bit-identical to Phase 1 (same base speed via `systems::step_toward`, same arrival eps).

use crate::combat;
use crate::components::{EntityKind, Faction, InputSource, Order, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::flow_field::FlowFieldCache;
use crate::systems;
use crate::terrain::Terrain;

/// Find the nearest alive friendly (`same faction`) building's position, or `Vec2::ZERO` if
/// there is none. Deterministic: iterates in stable index order and breaks ties toward the
/// lowest index (the `<` comparison never replaces an equal-distance earlier candidate).
fn nearest_friendly_building(world: &World, from: Vec2, faction: Faction) -> Vec2 {
    let mut best: Option<(Fixed, Vec2)> = None;
    let n = world.capacity();
    for j in 0..n {
        if !world.is_index_alive(j) {
            continue;
        }
        if world.kind[j] != EntityKind::Building || world.faction[j] != faction {
            continue;
        }
        let d = (world.pos[j] - from).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, world.pos[j])),
        }
    }
    match best {
        Some((_, p)) => p,
        None => Vec2::ZERO,
    }
}

/// The move speed for a unit this tick, derived from its suppression (`combat`):
/// pinned (zero) at/above [`combat::SUPPRESSION_PIN`], half base speed while any suppression
/// lingers, full [`systems::MOVE_SPEED`] when clean.
fn move_speed(suppression: Fixed) -> Fixed {
    if suppression >= combat::SUPPRESSION_PIN {
        Fixed::ZERO
    } else if suppression > Fixed::ZERO {
        systems::MOVE_SPEED / Fixed::from_int(2)
    } else {
        systems::MOVE_SPEED
    }
}

/// Advance every order-driven (non-embodied) unit one tick according to its order, stance,
/// suppression, and retreat trigger.
///
/// This is the sole order-driven mover and the literal executor
/// (invariant #3): a unit does *exactly* its order, with the only "reactions" being ones the
/// player pre-programmed (the retreat trigger). For an unsuppressed unit the `MoveTo`/`Idle`
/// behaviour is bit-identical to Phase 1 (same base speed via [`systems::step_toward`], same
/// arrival epsilon), so the determinism suite is unchanged.
pub fn order_system(world: &mut World, terrain: &Terrain) {
    let _ = terrain;
    let n = world.capacity();
    // One flow-field cache for the whole tick: units sharing a goal share a single build. It is
    // local to this call (dropped at tick end), so it is not sim state and never enters the
    // checksum; what each unit samples is bit-identical to building its own field.
    let mut cache = FlowFieldCache::new();
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        // Buildings don't move; the order executor ignores them.
        if world.kind[i] == EntityKind::Building {
            continue;
        }
        // Possessed units are driven by live player input, not orders (invariant #5).
        if world.input_source[i] == InputSource::Embodied {
            continue;
        }

        // Retreat trigger (D23): the player pre-programmed "fall back below X% HP". Install a
        // FallBack toward the nearest friendly building (or the origin) — once, not every tick.
        let threshold = world.retreat_below[i];
        if threshold > Fixed::ZERO
            && world.health[i].fraction() < threshold
            && !matches!(world.order[i], Order::FallBack(_))
        {
            let rally = nearest_friendly_building(world, world.pos[i], world.faction[i]);
            world.order[i] = Order::FallBack(rally);
        }

        // Suppression sets the move speed for this tick. When clean, use step_toward (base
        // speed) exactly as Phase 1 did so replays stay bit-identical.
        let suppression = world.suppression[i];

        match world.order[i] {
            Order::Idle | Order::HoldPosition => {
                world.vel[i] = Vec2::ZERO;
            }
            Order::MoveTo(target) => {
                if step(world, &mut cache, i, target, suppression) {
                    world.order[i] = Order::Idle;
                }
            }
            // Move to the point, then hold there (combat engages en route / on arrival).
            Order::AttackMove(target) => {
                if step(world, &mut cache, i, target, suppression) {
                    world.order[i] = Order::HoldPosition;
                }
            }
            // Retreat to the rally point and stay there. FallBack is TERMINAL: on arrival the
            // unit keeps the FallBack order (holding, vel zeroed by `step`) rather than flipping
            // to HoldPosition — otherwise, while still below the retreat threshold, the trigger
            // above would re-install FallBack every tick (the order would thrash forever). The
            // `!matches!(.., FallBack)` guard above relies on the order STAYING FallBack here.
            Order::FallBack(target) => {
                step(world, &mut cache, i, target, suppression);
            }
            // Bounce between the two legs forever.
            Order::Patrol { a, b, toward_b } => {
                let target = if toward_b { b } else { a };
                if step(world, &mut cache, i, target, suppression) {
                    world.order[i] = Order::Patrol {
                        a,
                        b,
                        toward_b: !toward_b,
                    };
                }
            }
        }
    }
}

/// Step unit `i` toward `target`, honouring suppression. An unsuppressed unit takes the exact
/// Phase 1 base-speed path ([`systems::step_toward`]) so determinism is preserved; otherwise
/// the suppression-derived speed (half, or zero/pinned) is used.
#[inline]
fn step(
    world: &mut World,
    cache: &mut FlowFieldCache,
    i: usize,
    target: Vec2,
    suppression: Fixed,
) -> bool {
    if suppression == Fixed::ZERO {
        systems::step_toward(world, cache, i, target)
    } else {
        systems::step_toward_speed(world, cache, i, target, move_speed(suppression))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Health;
    use crate::ecs::World;
    use crate::terrain::Terrain;

    fn world_with_unit() -> (World, usize) {
        let mut w = World::new();
        let e = w.spawn();
        (w, e.index as usize)
    }

    fn run(world: &mut World, ticks: usize) {
        let terrain = Terrain::default();
        for _ in 0..ticks {
            order_system(world, &terrain);
        }
    }

    #[test]
    fn idle_unit_stays_still_with_zero_velocity() {
        let (mut w, i) = world_with_unit();
        let start = w.pos[i];
        run(&mut w, 20);
        assert_eq!(w.pos[i], start);
        assert_eq!(w.vel[i], Vec2::ZERO);
    }

    #[test]
    fn moveto_reaches_target_then_idles() {
        let (mut w, i) = world_with_unit();
        let target = Vec2::new(Fixed::from_int(10), Fixed::from_int(5));
        w.order[i] = Order::MoveTo(target);
        run(&mut w, 400);
        assert!(
            (w.pos[i] - target).len_sq() <= systems::ARRIVE_EPS_SQ,
            "unit stalled at {:?}, target {:?}",
            w.pos[i],
            target
        );
        assert_eq!(w.order[i], Order::Idle);
        assert_eq!(w.vel[i], Vec2::ZERO);
    }

    #[test]
    fn embodied_unit_is_not_moved_by_orders() {
        let (mut w, i) = world_with_unit();
        w.order[i] = Order::MoveTo(Vec2::new(Fixed::from_int(50), Fixed::ZERO));
        w.input_source[i] = InputSource::Embodied;
        let before = w.pos[i];
        run(&mut w, 20);
        assert_eq!(w.pos[i], before);
    }

    #[test]
    fn building_is_not_moved_by_orders() {
        let (mut w, i) = world_with_unit();
        w.kind[i] = EntityKind::Building;
        w.order[i] = Order::MoveTo(Vec2::new(Fixed::from_int(50), Fixed::ZERO));
        let before = w.pos[i];
        run(&mut w, 20);
        assert_eq!(w.pos[i], before);
    }

    #[test]
    fn patrol_bounces_between_legs() {
        let (mut w, i) = world_with_unit();
        let a = Vec2::ZERO;
        let b = Vec2::new(Fixed::from_int(8), Fixed::ZERO);
        w.order[i] = Order::Patrol {
            a,
            b,
            toward_b: true,
        };
        // Long enough to reach b and flip the leg.
        run(&mut w, 200);
        match w.order[i] {
            Order::Patrol { toward_b, .. } => {
                assert!(!toward_b, "after reaching b the leg should flip toward a");
            }
            other => panic!("patrol order was replaced: {other:?}"),
        }
        // It must have arrived at b at the flip moment, then started heading back toward a:
        // x should be strictly decreasing from b once it turns around.
        let x_after_flip = w.pos[i].x;
        run(&mut w, 1);
        assert!(
            w.pos[i].x < x_after_flip,
            "should be heading back toward a (decreasing x)"
        );
    }

    #[test]
    fn retreat_trigger_installs_fallback_to_origin_when_no_building() {
        let (mut w, i) = world_with_unit();
        // Start away from origin with a benign order.
        w.pos[i] = Vec2::new(Fixed::from_int(20), Fixed::ZERO);
        w.order[i] = Order::HoldPosition;
        w.retreat_below[i] = Fixed::HALF;
        // Health below half.
        w.health[i] = Health {
            cur: Fixed::from_int(40),
            max: Fixed::from_int(100),
        };
        let before = w.pos[i];
        run(&mut w, 1);
        match w.order[i] {
            Order::FallBack(rally) => assert_eq!(rally, Vec2::ZERO),
            other => panic!("expected FallBack(ZERO), got {other:?}"),
        }
        // It moves toward the origin (x decreasing from +20).
        assert!(w.pos[i].x < before.x, "should fall back toward origin");
    }

    #[test]
    fn retreat_trigger_rallies_to_nearest_friendly_building() {
        let mut w = World::new();
        let unit = w.spawn();
        let bldg = w.spawn();
        let ui = unit.index as usize;
        let bi = bldg.index as usize;
        let rally = Vec2::new(Fixed::from_int(5), Fixed::from_int(5));
        w.kind[bi] = EntityKind::Building;
        w.faction[bi] = Faction::Player;
        w.pos[bi] = rally;
        w.faction[ui] = Faction::Player;
        w.pos[ui] = Vec2::new(Fixed::from_int(30), Fixed::from_int(30));
        w.order[ui] = Order::HoldPosition;
        w.retreat_below[ui] = Fixed::HALF;
        w.health[ui] = Health {
            cur: Fixed::from_int(10),
            max: Fixed::from_int(100),
        };
        run(&mut w, 1);
        match w.order[ui] {
            Order::FallBack(r) => assert_eq!(r, rally),
            other => panic!("expected FallBack to building, got {other:?}"),
        }
    }

    #[test]
    fn fully_suppressed_unit_does_not_advance() {
        let (mut w, i) = world_with_unit();
        w.pos[i] = Vec2::ZERO;
        w.order[i] = Order::MoveTo(Vec2::new(Fixed::from_int(50), Fixed::ZERO));
        w.suppression[i] = Fixed::ONE; // >= SUPPRESSION_PIN → pinned
        run(&mut w, 10);
        assert_eq!(w.pos[i], Vec2::ZERO, "pinned unit must not advance");
        assert_eq!(w.vel[i], Vec2::ZERO);
        assert_eq!(
            w.order[i],
            Order::MoveTo(Vec2::new(Fixed::from_int(50), Fixed::ZERO)),
            "order not complete while pinned"
        );
    }

    #[test]
    fn half_suppressed_unit_moves_slower_than_clean_one() {
        // Two units, same start/target; one suppressed (but not pinned), one clean.
        let mut w = World::new();
        let clean = w.spawn();
        let supp = w.spawn();
        let ci = clean.index as usize;
        let si = supp.index as usize;
        let target = Vec2::new(Fixed::from_int(50), Fixed::ZERO);
        w.order[ci] = Order::MoveTo(target);
        w.order[si] = Order::MoveTo(target);
        // A suppression strictly between 0 and PIN → half speed. Use a value comfortably
        // below SUPPRESSION_PIN (now 1/2 — D30) so this exercises the "slowed but not pinned"
        // path regardless of small pin-threshold moves.
        w.suppression[si] = Fixed::from_ratio(1, 4);
        assert!(
            w.suppression[si] < combat::SUPPRESSION_PIN && w.suppression[si] > Fixed::ZERO,
            "test fixture must be slowed-but-not-pinned"
        );
        run(&mut w, 5);
        assert!(
            w.pos[ci].x > w.pos[si].x,
            "clean unit ({:?}) should outpace the half-suppressed one ({:?})",
            w.pos[ci].x,
            w.pos[si].x
        );
        // And it should be strictly behind, not stalled.
        assert!(
            w.pos[si].x > Fixed::ZERO,
            "half-suppressed unit must still move"
        );
    }

    #[test]
    fn fallback_is_terminal_and_retreat_trigger_does_not_refire() {
        // Regression: once a retreating unit reaches its rally it must KEEP the FallBack order
        // (holding there). Earlier it flipped to HoldPosition, which — while still below the
        // retreat threshold — let the trigger re-install FallBack every tick (order thrash).
        let (mut w, i) = world_with_unit();
        w.pos[i] = Vec2::new(Fixed::from_int(2), Fixed::ZERO); // close, arrives quickly
        w.order[i] = Order::HoldPosition;
        w.retreat_below[i] = Fixed::HALF;
        w.health[i] = Health {
            cur: Fixed::from_int(10), // stays below half the whole time (no healing)
            max: Fixed::from_int(100),
        };
        // Run well past arrival at the origin rally.
        run(&mut w, 60);
        // Order is still FallBack to the origin — not HoldPosition, not thrashing.
        match w.order[i] {
            Order::FallBack(rally) => assert_eq!(rally, Vec2::ZERO),
            other => panic!("FallBack should be terminal, got {other:?}"),
        }
        // It arrived and is holding (snapped to the rally, zero velocity).
        assert_eq!(w.pos[i], Vec2::ZERO);
        assert_eq!(w.vel[i], Vec2::ZERO);
        // And the order is byte-stable across further ticks (no flip → no checksum thrash).
        let order_before = w.order[i];
        run(&mut w, 1);
        assert_eq!(
            w.order[i], order_before,
            "order must not thrash once retreated"
        );
    }
}

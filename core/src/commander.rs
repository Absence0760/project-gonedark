//! The enemy commander — a deterministic, *commander-level* scripted AI (invariant #3).
//!
//! This is the strategic brain a human opponent would be: it surveys the (identical,
//! checksummed) world and **issues orders** — exactly the orders a player issues through the
//! command UI (`AttackMove` / `SetOrder` / `SetStance` / `Build` / `QueueProduction`). It does
//! **not** give units autonomous smarts. Units remain pure literal executors (invariant #3,
//! [`orders`](crate::orders)): a unit still does *exactly* its last order, every tick, forever.
//! All "intelligence" here is the commander *choosing* which order to hand a unit, never the
//! unit deciding for itself. A *commander* issuing orders is explicitly distinct from, and
//! allowed alongside, the literal-executor rule.
//!
//! Determinism (invariants #1, #7):
//! - **No floats.** Every comparison is on squared `Fixed` magnitudes ([`Vec2::len_sq`]) — no
//!   `sqrt`/`normalize`/transcendental. The determinism guard greps this file (incl. tests).
//! - **Stable iteration + tie-breaks.** Every scan walks the world in `0..capacity()` index
//!   order; "nearest" ties break toward the lowest index (`<` never replaces an equal-distance
//!   earlier candidate), so the produced command list is a pure function of `(world, territory,
//!   resources, rng-state, faction, tick)`.
//! - **Own RNG stream.** The commander draws from a RNG owned by the *host* (`engine::Game`),
//!   seeded `sim_seed ^ faction`, **never** `Sim::rng()` (that stream is folded into the
//!   checksum; a host-side draw would advance it and desync). The host pushes the returned
//!   commands into the same lockstep stream player commands travel, so they are applied
//!   bit-identically on every peer — the commander itself stays peer-agnostic.
//!
//! The host calls [`commander_orders`] on a `tick % PERIOD == 0` cadence (see
//! [`COMMANDER_PERIOD`]); on off-ticks it issues nothing. Returning a `Vec<Command>` (not
//! mutating the world) keeps it a *pure planner* — the sim still applies every command through
//! the one authoritative `Sim::apply` path.

use crate::components::{EntityKind, Faction, Order, Stance, UnitKind, Vec2};
use crate::ecs::World;
use crate::economy::{self, Resources};
use crate::fixed::Fixed;
use crate::rng::Rng;
use crate::sim::Command;
use crate::territory::Territory;

/// How often (in ticks) the host invokes the commander. 60 ticks = 1 s at the locked 60 Hz
/// ([`crate::sim::TICK_HZ`]): a deliberate, human-cadence re-plan, not a per-tick micro. Cheap
/// (a per-second linear scan), and slow enough that order churn reads as decisions, not jitter.
pub const COMMANDER_PERIOD: u64 = 60;

/// Radius (world units) within which a unit is considered "already committed" to a control
/// point and is not re-tasked. Matches the territory capture radius so a unit sitting on a
/// point it is capturing is left to finish the job. Squared at the use site (no sqrt).
const POINT_COMMIT_RADIUS: Fixed = crate::territory::CAPTURE_RADIUS;

/// Max production backlog the commander will queue at one camp. Keeps some purse free for the
/// rest of the plan and stops a stalled queue from hoarding resources.
const MAX_QUEUE_DEPTH: usize = 2;

/// Resource cushion kept in reserve when deciding to splurge on a Heavy — only buy the pricey
/// unit when comfortably flush, so the commander doesn't bankrupt itself on one bruiser.
const RESERVE: i64 = economy::RIFLEMAN_COST;

/// Survey the world and return the orders to feed the lockstep stream this tick — possibly empty
/// (nothing affordable, no idle units, no targets). The host owns the RNG (its own stream,
/// seeded `sim_seed ^ faction`) and passes it in by `&mut`; everything else is a read-only view
/// of already-checksummed sim state. The caller pushes the result into the same `commands` Vec
/// that drives `drive_lockstep`, *before* the lockstep step.
///
/// Behavior loop (all "only existing order/economy commands", invariant #3):
/// 1. **Reinforce.** For each built friendly camp, if the faction can afford a unit, queue one
///    (`QueueProduction`). Heavy when flush, else Rifleman — pure resource thresholds, no float.
/// 2. **Capture.** Idle / freshly-produced units not already committed to a point are sent to
///    the nearest neutral-or-enemy control point (`AttackMove` onto it) — taking ground is how
///    you out-produce the player.
/// 3. **Attack.** Units with no point to take are pointed at the nearest hostile force
///    (`AttackMove` toward the nearest enemy unit/building) so the line keeps pressing.
/// 4. **Posture.** Any unit on `HoldFire` is bumped to `ReturnFire` so the commander's army
///    actually shoots back (a one-shot stance fix; idempotent thereafter).
pub fn commander_orders(
    world: &World,
    territory: &Territory,
    resources: &Resources,
    rng: &mut Rng,
    faction: Faction,
    tick: u64,
) -> Vec<Command> {
    let _ = tick; // cadence is the caller's gate; kept in the signature for future phasing.
    let mut commands = Vec::new();

    // --- 1. Reinforce: spend banked resources on production at each built camp. ----------------
    // A running purse so we never over-commit beyond what we can afford THIS plan (the sim's
    // `try_spend` is the final authority, but planning against a local purse keeps us from
    // queueing five units we can pay for once).
    let mut purse = resources.get(faction);
    for i in 0..world.capacity() {
        if !world.is_index_alive(i)
            || world.kind[i] != EntityKind::Building
            || world.faction[i] != faction
        {
            continue;
        }
        let b = &world.building[i];
        // Only a finished camp produces.
        if b.build_ticks_left != 0 {
            continue;
        }
        // Don't pile the queue arbitrarily deep — at most a small backlog so resources also fund
        // captures/expansion, and so a stalled front item doesn't hoard the whole purse.
        if b.queue.len() >= MAX_QUEUE_DEPTH {
            continue;
        }
        let Some(camp) = world.entity(i) else {
            continue;
        };
        // Flush → buy a Heavy (the expensive bruiser); otherwise the cheap, spammable Rifleman.
        let unit = if purse >= economy::HEAVY_COST + RESERVE {
            UnitKind::Heavy
        } else {
            UnitKind::Rifleman
        };
        let cost = economy::unit_cost(unit);
        if purse >= cost {
            purse -= cost;
            commands.push(Command::QueueProduction { camp, unit });
        }
    }

    // --- 2 & 3. Task idle units: capture the nearest open point, else press the nearest foe. ---
    for i in 0..world.capacity() {
        if !world.is_index_alive(i)
            || world.kind[i] != EntityKind::Unit
            || world.faction[i] != faction
        {
            continue;
        }
        // Posture fix: an idle army that won't shoot is useless. Bump HoldFire → ReturnFire once.
        if world.stance[i] == Stance::HoldFire {
            if let Some(e) = world.entity(i) {
                commands.push(Command::SetStance {
                    entity: e,
                    stance: Stance::ReturnFire,
                });
            }
        }

        // Only (re-)task units free to take a new objective: Idle / HoldPosition. A unit mid-
        // MoveTo/AttackMove/Patrol/FallBack is left to finish its current order (re-issuing every
        // period would thrash it).
        if !matches!(world.order[i], Order::Idle | Order::HoldPosition) {
            continue;
        }
        let pos = world.pos[i];

        // Already standing on a not-yet-ours point? Leave it to capture (don't re-issue).
        if sitting_on_open_point(territory, pos, faction) {
            continue;
        }

        let Some(e) = world.entity(i) else {
            continue;
        };

        // Prefer taking ground: nearest neutral/enemy control point.
        if let Some(target) = nearest_open_point(territory, pos, faction) {
            commands.push(Command::AttackMove { entity: e, target });
            continue;
        }

        // No point to take (we own them all, or there are none) → press the nearest hostile force.
        if let Some(target) = nearest_hostile(world, pos, faction) {
            commands.push(Command::AttackMove { entity: e, target });
        }
    }

    // The RNG is part of the contract (own stream, seeded `sim_seed ^ faction`) so the commander
    // can later make randomized-but-deterministic choices without ever touching `Sim::rng`. Today
    // the plan is fully deterministic from world state, so we don't draw — but we keep the seam.
    let _ = rng;

    commands
}

/// Is `pos` within the commit radius of a control point this `faction` does NOT yet own? Such a
/// unit is left alone to finish capturing (re-tasking it would interrupt its own capture).
/// Squared-magnitude comparison only (no sqrt). Stable: any matching point in index order.
fn sitting_on_open_point(territory: &Territory, pos: Vec2, faction: Faction) -> bool {
    let r_sq = POINT_COMMIT_RADIUS * POINT_COMMIT_RADIUS;
    territory
        .points
        .iter()
        .any(|p| p.owner != faction && (p.pos - pos).len_sq() <= r_sq)
}

/// Nearest control point not owned by `faction` (neutral or enemy), by squared distance from
/// `pos`. `None` if the faction owns every point (or there are none). Deterministic: stable
/// vector order, ties break toward the earliest point (`<` never displaces an equal-distance
/// earlier one).
fn nearest_open_point(territory: &Territory, pos: Vec2, faction: Faction) -> Option<Vec2> {
    let mut best: Option<(Fixed, Vec2)> = None;
    for p in &territory.points {
        if p.owner == faction {
            continue;
        }
        let d = (p.pos - pos).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, p.pos)),
        }
    }
    best.map(|(_, t)| t)
}

/// Nearest hostile (different, non-`Neutral` faction) entity — unit OR building — to `pos`, by
/// squared distance. `None` if there is no hostile alive. Deterministic: stable index-order
/// scan, ties break toward the lowest index.
fn nearest_hostile(world: &World, pos: Vec2, faction: Faction) -> Option<Vec2> {
    let mut best: Option<(Fixed, Vec2)> = None;
    for j in 0..world.capacity() {
        if !world.is_index_alive(j) {
            continue;
        }
        let f = world.faction[j];
        if f == faction || f == Faction::Neutral {
            continue;
        }
        let d = (world.pos[j] - pos).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, world.pos[j])),
        }
    }
    best.map(|(_, t)| t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Building, BuildingKind, Health};
    use crate::ecs::{Entity, World};
    use crate::territory::ControlPoint;

    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    fn spawn_unit(world: &mut World, faction: Faction, pos: Vec2) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.kind[i] = EntityKind::Unit;
        world.faction[i] = faction;
        world.pos[i] = pos;
        world.order[i] = Order::Idle;
        world.stance[i] = Stance::ReturnFire;
        e
    }

    fn spawn_built_camp(world: &mut World, faction: Faction, pos: Vec2) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.kind[i] = EntityKind::Building;
        world.faction[i] = faction;
        world.pos[i] = pos;
        world.health[i] = Health::full(Fixed::from_int(1000));
        world.building[i] = Building {
            kind: BuildingKind::Camp,
            level: 0,
            build_ticks_left: 0, // finished → can produce
            queue: Vec::new(),
        };
        e
    }

    /// Same (seed, tick, world, territory, resources) ⇒ identical command list, twice over.
    #[test]
    fn deterministic_for_identical_inputs() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        spawn_unit(&mut world, Faction::Enemy, at(22, 3));
        spawn_built_camp(&mut world, Faction::Enemy, at(30, 0));
        spawn_unit(&mut world, Faction::Player, at(-5, 0)); // a hostile to target
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let res = Resources::new(500);

        let mut rng_a = Rng::new(123);
        let a = commander_orders(&world, &terr, &res, &mut rng_a, Faction::Enemy, 60);
        let mut rng_b = Rng::new(123);
        let b = commander_orders(&world, &terr, &res, &mut rng_b, Faction::Enemy, 60);

        assert_eq!(a.len(), b.len(), "same inputs → same number of commands");
        // Commands are Copy/Debug; compare their debug forms field-for-field.
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(format!("{x:?}"), format!("{y:?}"), "command streams diverged");
        }
    }

    /// An idle unit + a neutral control point ⇒ the commander issues an AttackMove ONTO the point
    /// (capture order), not toward the enemy.
    #[test]
    fn idle_unit_gets_capture_order_for_neutral_point() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        // A distant player unit also exists, but the open point is the priority target.
        spawn_unit(&mut world, Faction::Player, at(-50, 0));
        let point = at(5, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(point)],
        };
        let res = Resources::new(0); // no money → no production noise

        let mut rng = Rng::new(1);
        let cmds = commander_orders(&world, &terr, &res, &mut rng, Faction::Enemy, 60);

        let captured = cmds.iter().any(|c| {
            matches!(c, Command::AttackMove { entity, target }
                if *entity == u && *target == point)
        });
        assert!(
            captured,
            "idle unit should be sent to capture the open point: {cmds:?}"
        );
        // And it must NOT have been pointed at the far player instead.
        assert!(
            !cmds.iter().any(
                |c| matches!(c, Command::AttackMove { target, .. } if *target == at(-50, 0))
            ),
            "the open point outranks the distant foe as a target"
        );
    }

    /// With no open point but a hostile present, the idle unit is pointed at the nearest foe.
    #[test]
    fn idle_unit_attacks_nearest_foe_when_no_open_point() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        let near = at(10, 0);
        let far = at(-30, 0);
        spawn_unit(&mut world, Faction::Player, near);
        spawn_unit(&mut world, Faction::Player, far);
        // The only point is already owned by us → not "open", so step 3 (attack) applies.
        let terr = Territory {
            points: vec![ControlPoint {
                pos: at(0, 0),
                owner: Faction::Enemy,
                progress: Fixed::ZERO,
            }],
        };
        let res = Resources::new(0);

        let mut rng = Rng::new(1);
        let cmds = commander_orders(&world, &terr, &res, &mut rng, Faction::Enemy, 60);
        let attacked_near = cmds.iter().any(|c| {
            matches!(c, Command::AttackMove { entity, target } if *entity == u && *target == near)
        });
        assert!(attacked_near, "should target the NEAREST foe: {cmds:?}");
    }

    /// Nearest-foe targeting picks the closer of two by squared distance, with a stable tie-break
    /// toward the lower index when distances are exactly equal.
    #[test]
    fn targeting_picks_nearest_with_stable_tiebreak() {
        // Two equidistant foes: the lower-index one wins.
        let mut world = World::new();
        let _u = spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        let first = spawn_unit(&mut world, Faction::Player, at(10, 0)); // index lower
        let _second = spawn_unit(&mut world, Faction::Player, at(-10, 0)); // same dist
        let chosen = nearest_hostile(&world, at(0, 0), Faction::Enemy).unwrap();
        assert_eq!(
            chosen,
            world.pos[first.index as usize],
            "equal distance → earliest index wins (stable tie-break)"
        );

        // And a strictly closer foe is preferred regardless of order.
        let mut w2 = World::new();
        spawn_unit(&mut w2, Faction::Player, at(40, 0));
        spawn_unit(&mut w2, Faction::Player, at(5, 0));
        let chosen2 = nearest_hostile(&w2, at(0, 0), Faction::Enemy).unwrap();
        assert_eq!(chosen2, at(5, 0), "strictly nearer foe wins");
    }

    /// Production is queued only when the faction can afford a unit; never when broke.
    #[test]
    fn queues_production_only_when_affordable() {
        let mut world = World::new();
        let camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));

        // Broke: no production command at all.
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let broke = commander_orders(
            &world,
            &terr,
            &Resources::new(economy::RIFLEMAN_COST - 1),
            &mut rng,
            Faction::Enemy,
            60,
        );
        assert!(
            !broke
                .iter()
                .any(|c| matches!(c, Command::QueueProduction { .. })),
            "must not queue when it can't afford a unit: {broke:?}"
        );

        // Exactly a Rifleman's worth → queue one Rifleman.
        let mut rng = Rng::new(1);
        let afford = commander_orders(
            &world,
            &terr,
            &Resources::new(economy::RIFLEMAN_COST),
            &mut rng,
            Faction::Enemy,
            60,
        );
        let queued_rifle = afford.iter().any(|c| {
            matches!(c, Command::QueueProduction { camp: cc, unit: UnitKind::Rifleman }
                if *cc == camp)
        });
        assert!(
            queued_rifle,
            "should queue a Rifleman when just affordable: {afford:?}"
        );
        assert!(
            !afford.iter().any(
                |c| matches!(c, Command::QueueProduction { unit: UnitKind::Heavy, .. })
            ),
            "not flush enough for a Heavy"
        );
    }

    /// When flush, the commander splurges on the Heavy bruiser.
    #[test]
    fn queues_heavy_when_flush() {
        let mut world = World::new();
        let _camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(economy::HEAVY_COST + RESERVE),
            &mut rng,
            Faction::Enemy,
            60,
        );
        assert!(
            cmds.iter().any(
                |c| matches!(c, Command::QueueProduction { unit: UnitKind::Heavy, .. })
            ),
            "flush commander should buy a Heavy: {cmds:?}"
        );
    }

    /// An unbuilt (still-constructing) camp never produces.
    #[test]
    fn unbuilt_camp_does_not_produce() {
        let mut world = World::new();
        let e = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        world.building[e.index as usize].build_ticks_left = 100; // under construction
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(10_000),
            &mut rng,
            Faction::Enemy,
            60,
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::QueueProduction { .. })),
            "a camp under construction must not produce"
        );
    }

    /// A unit standing on the open point it is capturing is NOT re-tasked (don't interrupt it).
    #[test]
    fn unit_on_open_point_is_not_retasked() {
        let mut world = World::new();
        let _u = spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))], // unit sits exactly on it
        };
        let mut rng = Rng::new(1);
        let cmds =
            commander_orders(&world, &terr, &Resources::new(0), &mut rng, Faction::Enemy, 60);
        assert!(
            !cmds.iter().any(|c| matches!(c, Command::AttackMove { .. })),
            "a unit already on its capture point should be left alone: {cmds:?}"
        );
    }

    /// A unit already mid-order (AttackMove) is not re-tasked every period (no thrash).
    #[test]
    fn busy_unit_is_not_retasked() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.order[u.index as usize] = Order::AttackMove(at(5, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(5, 0))],
        };
        let mut rng = Rng::new(1);
        let cmds =
            commander_orders(&world, &terr, &Resources::new(0), &mut rng, Faction::Enemy, 60);
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { entity, .. } if *entity == u)),
            "a unit already executing an AttackMove must not be re-issued: {cmds:?}"
        );
    }

    /// A HoldFire unit is bumped to ReturnFire so the army actually fights.
    #[test]
    fn hold_fire_unit_is_bumped_to_return_fire() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.stance[u.index as usize] = Stance::HoldFire;
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let cmds =
            commander_orders(&world, &terr, &Resources::new(0), &mut rng, Faction::Enemy, 60);
        assert!(
            cmds.iter().any(|c| matches!(c, Command::SetStance { entity, stance: Stance::ReturnFire }
                if *entity == u)),
            "a HoldFire unit should be set to ReturnFire: {cmds:?}"
        );
    }

    /// The commander only ever touches its own faction's units/camps — never the player's.
    #[test]
    fn never_orders_other_factions() {
        let mut world = World::new();
        let player_unit = spawn_unit(&mut world, Faction::Player, at(0, 0));
        let player_camp = spawn_built_camp(&mut world, Faction::Player, at(3, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(10, 0))],
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(10_000),
            &mut rng,
            Faction::Enemy,
            60,
        );
        for c in &cmds {
            match c {
                Command::AttackMove { entity, .. }
                | Command::SetStance { entity, .. }
                | Command::SetOrder { entity, .. } => {
                    assert_ne!(*entity, player_unit, "must not order a player unit");
                }
                Command::QueueProduction { camp, .. } => {
                    assert_ne!(*camp, player_camp, "must not produce at a player camp");
                }
                _ => {}
            }
        }
    }
}

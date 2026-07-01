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
use crate::detection::Tell;
use crate::ecs::World;
use crate::economy::{self, Resources};
use crate::fixed::Fixed;
use crate::mission_tuning::Difficulty;
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

// The production backlog cap and the Heavy-purchase reserve are no longer fixed constants: they
// are **difficulty knobs** ([`mission_tuning::DifficultyParams`]), so a tier scales how deep the
// commander queues and how big a cushion it keeps. The default tier ([`Difficulty::Veteran`])
// returns the original values (`max_queue_depth = 2`, `heavy_reserve = RIFLEMAN_COST`), so the
// default scenes' command streams are byte-identical (see `mission_tuning`).

/// Tunable knobs for the commander — a *mechanism*, not a frozen design (the D23/D26/D33 house
/// style). Defaults reproduce the original, golden-checksum-stable behavior **byte-for-byte**, so
/// adding a knob never perturbs the default `phase2`/`stress`/demo command streams.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CommanderConfig {
    /// How aggressively this commander plays — a deterministic difficulty tier
    /// ([`mission_tuning::Difficulty`]) that scales the planner's **choices** (production backlog
    /// depth, the Heavy reserve, and the army re-plan cadence), never its **knowledge**. A harder
    /// tier issues orders sooner and spends more freely; it reads *nothing* about the player going
    /// dark — invariant #6 is structural, not a discipline (the gone-dark consult stays gated
    /// solely by [`hunt_embodied`](Self::hunt_embodied), independent of tier).
    ///
    /// **Defaults to [`Veteran`](Difficulty::Veteran)**, whose knobs reproduce the commander's
    /// original constants exactly, so the default scenes' lockstep/checksum streams are untouched.
    pub difficulty: Difficulty,

    /// When `true`, the commander may **consult the detection channel** and chase a hostile that
    /// has "gone dark" (embodied). It reads ONLY the `tells` the caller derived from
    /// [`detection::detectable_embodiment`](crate::detection::detectable_embodiment) for *this*
    /// faction as observer — so it learns exactly what detection honestly permits (range +
    /// line-of-sight bounded, with the `Subtle` linger) and **nothing more**: in `Hidden` mode, or
    /// out of range / no LoS, the slice is empty and the commander reacts to nothing it could not
    /// legitimately know. That structural bound — the commander cannot peek at `&World` for embodied
    /// enemies itself, only consume the channel — is the point (invariant #6 fairness, "no
    /// omniscient peek").
    ///
    /// **Defaults `false`** so the default scenes' lockstep command streams stay byte-identical;
    /// enable it per-scene/per-difficulty to make the AI hunt a gone-dark player.
    pub hunt_embodied: bool,
}

/// Survey the world and return the orders to feed the lockstep stream this tick — possibly empty
/// (nothing affordable, no idle units, no targets). The host owns the RNG (its own stream,
/// seeded `sim_seed ^ faction`) and passes it in by `&mut`; everything else is a read-only view
/// of already-checksummed sim state. The caller pushes the result into the same `commands` Vec
/// that drives `drive_lockstep`, *before* the lockstep step.
///
/// `config` gates optional behavior; `tells` is the detection channel's output for `faction` as
/// observer (the caller derives it from [`detection::detectable_embodiment`](crate::detection)).
/// With `CommanderConfig::default()` and `tells == &[]`, the returned command list is **identical,
/// byte-for-byte, to the original commander** — the default scenes' checksum streams are untouched.
///
/// Behavior loop (all "only existing order/economy commands", invariant #3):
/// 1. **Reinforce.** For each built friendly camp, if the faction can afford a unit, queue one
///    (`QueueProduction`). Heavy when flush, else Rifleman — pure resource thresholds, no float.
/// 2. **Hunt the dark** *(only when `config.hunt_embodied`)*. If a hostile has gone dark
///    (embodied) within what the detection channel HONESTLY reveals (a non-empty `tells`), a free
///    unit is pressed toward its nearest tell's (last-seen) position ABOVE taking ground — a
///    gone-dark player is the juiciest target. Empty `tells` (out of range / no LoS / `Hidden`) ⇒
///    no reaction, so the AI never knows more than detection grants (invariant #6, no omniscient
///    peek). Off by default → no effect on the default streams.
/// 3. **Capture.** Idle / freshly-produced units not already committed to a point are sent to
///    the nearest neutral-or-enemy control point (`AttackMove` onto it) — taking ground is how
///    you out-produce the player.
/// 4. **Attack.** Units with no point to take are pointed at the nearest hostile force
///    (`AttackMove` toward the nearest enemy unit/building) so the line keeps pressing.
/// 5. **Posture.** Any unit on `HoldFire` is bumped to `FireAtWill` so the commander's army
///    actually engages (a one-shot stance fix; idempotent thereafter). `ReturnFire` would not do:
///    a `HoldFire`/`ReturnFire` unit only shoots once *it* is hit, so a defending line would never
///    open up on an attacker — it must `FireAtWill` to fight on its own.
#[allow(clippy::too_many_arguments)] // honest read-only inputs; bundling them buys no clarity
pub fn commander_orders(
    world: &World,
    territory: &Territory,
    resources: &Resources,
    rng: &mut Rng,
    config: &CommanderConfig,
    tells: &[Tell],
    faction: Faction,
    tick: u64,
) -> Vec<Command> {
    // Difficulty tier → the integer knobs that scale this plan (aggression / reserve / cadence).
    // The default tier (`Veteran`) returns the commander's original constants, so a default-config
    // call is byte-identical to the pre-difficulty commander.
    let params = config.difficulty.params();

    // Re-plan **cadence** (the `command_stride` knob): the army-tasking + posture pass runs only on
    // cycles where `cycle % stride == 0`, where `cycle = tick / COMMANDER_PERIOD` is a pure function
    // of sim state (so it is identical on every peer regardless of frame pacing). Stride `1`
    // (Veteran) ⇒ every cycle ⇒ the original behavior; a larger stride makes an easier commander
    // reconsider its orders less often. Reinforcement is intentionally *not* strided.
    let retask_this_cycle =
        params.command_stride <= 1 || (tick / COMMANDER_PERIOD).is_multiple_of(params.command_stride);

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
        // Don't pile the queue arbitrarily deep — at most the tier's small backlog so resources
        // also fund captures/expansion, and so a stalled front item doesn't hoard the whole purse.
        // (`max_queue_depth` is the difficulty **aggression** knob; Veteran = 2 as before.)
        if b.queue.len() >= params.max_queue_depth {
            continue;
        }
        let Some(camp) = world.entity(i) else {
            continue;
        };
        // Flush → buy a Heavy (the expensive bruiser); otherwise the cheap, spammable Rifleman. The
        // cushion is the difficulty **reserve / unit-mix** knob: a fat reserve keeps the mix rifle-
        // heavy, a zero reserve (Elite) buys the Heavy the instant it is affordable. Veteran's
        // reserve is one Rifleman — the original threshold.
        let unit = if purse >= economy::HEAVY_COST + params.heavy_reserve {
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
    // Gated by the difficulty cadence: an easier commander (stride > 1) skips re-tasking on
    // off-cycles, so its army reconsiders orders less often. At Veteran stride this runs every
    // cycle exactly as before.
    if retask_this_cycle {
        for i in 0..world.capacity() {
            if !world.is_index_alive(i)
                || world.kind[i] != EntityKind::Unit
                || world.faction[i] != faction
            {
                continue;
            }
            // Posture fix: an idle army that won't shoot is useless. Bump HoldFire → FireAtWill once
            // so the unit engages enemies in range on its own (ReturnFire would only ever shoot back
            // after being hit, never opening the fight — the AI-vs-AI first-shot deadlock).
            if world.stance[i] == Stance::HoldFire {
                if let Some(e) = world.entity(i) {
                    commands.push(Command::SetStance {
                        entity: e,
                        stance: Stance::FireAtWill,
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

            // Hunt the dark (config-gated, default OFF): a hostile that has gone dark (embodied) and is
            // HONESTLY detectable — i.e. present in `tells`, which the caller bounded to range + LoS via
            // the detection channel — is the priority target, above taking ground. `tells` is empty when
            // detection reveals nothing (out of range / no LoS / `Hidden`), so this is a no-op then: the
            // commander reacts only to what it could legitimately know (invariant #6, no omniscient peek).
            if config.hunt_embodied {
                if let Some(target) = nearest_tell(tells, pos) {
                    commands.push(Command::AttackMove { entity: e, target });
                    continue;
                }
            }

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

/// Nearest gone-dark tell to `pos` by squared distance (no sqrt). `None` for an empty slice.
/// Deterministic: stable slice order, ties break toward the earliest tell (`<` never displaces an
/// equal-distance earlier one) — exactly the tie-break the other "nearest" scans use. Reads only
/// the (presentation-derived but float-free) tell positions, never `&World` — so the commander's
/// gone-dark knowledge is bounded by the detection channel that produced `tells`.
fn nearest_tell(tells: &[Tell], pos: Vec2) -> Option<Vec2> {
    let mut best: Option<(Fixed, Vec2)> = None;
    for t in tells {
        let d = (t.pos - pos).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, t.pos)),
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Building, BuildingKind, Health, InputSource};
    use crate::detection::{detectable_embodiment, DetectionConfig, DetectionMemory, TellMode};
    use crate::ecs::{Entity, World};
    use crate::terrain::{Cover, Terrain};
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
            rally: None,
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
        let a = commander_orders(&world, &terr, &res, &mut rng_a, &CommanderConfig::default(), &[], Faction::Enemy, 60);
        let mut rng_b = Rng::new(123);
        let b = commander_orders(&world, &terr, &res, &mut rng_b, &CommanderConfig::default(), &[], Faction::Enemy, 60);

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
        let cmds = commander_orders(&world, &terr, &res, &mut rng, &CommanderConfig::default(), &[], Faction::Enemy, 60);

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
        let cmds = commander_orders(&world, &terr, &res, &mut rng, &CommanderConfig::default(), &[], Faction::Enemy, 60);
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
            &CommanderConfig::default(),
            &[],
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
            &CommanderConfig::default(),
            &[],
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
            // The default (Veteran) tier's Heavy reserve is one Rifleman — the original threshold.
            &Resources::new(economy::HEAVY_COST + economy::RIFLEMAN_COST),
            &mut rng,
            &CommanderConfig::default(),
            &[],
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
            &CommanderConfig::default(),
            &[],
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
            commander_orders(&world, &terr, &Resources::new(0), &mut rng, &CommanderConfig::default(), &[], Faction::Enemy, 60);
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
            commander_orders(&world, &terr, &Resources::new(0), &mut rng, &CommanderConfig::default(), &[], Faction::Enemy, 60);
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { entity, .. } if *entity == u)),
            "a unit already executing an AttackMove must not be re-issued: {cmds:?}"
        );
    }

    /// A HoldFire unit is bumped to FireAtWill so the army actually fights (engages on sight, not
    /// merely shoots back once hit — the latter would deadlock two opposing defensive lines).
    #[test]
    fn hold_fire_unit_is_bumped_to_fire_at_will() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.stance[u.index as usize] = Stance::HoldFire;
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let cmds =
            commander_orders(&world, &terr, &Resources::new(0), &mut rng, &CommanderConfig::default(), &[], Faction::Enemy, 60);
        assert!(
            cmds.iter().any(|c| matches!(c, Command::SetStance { entity, stance: Stance::FireAtWill }
                if *entity == u)),
            "a HoldFire unit should be set to FireAtWill: {cmds:?}"
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
            &CommanderConfig::default(),
            &[],
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

    // --- Gone-dark hunt (config-gated detection-channel consult) -----------------------------
    //
    // The commander may CONSULT the detection channel to chase a hostile that has gone dark
    // (embodied) — but only within what detection HONESTLY permits (range + LoS, or `Hidden` →
    // nothing). The behavior is gated behind `CommanderConfig::hunt_embodied`, default OFF, so the
    // default scenes' command streams stay byte-identical (no golden-checksum churn).

    /// Embodied (gone-dark) variant of `spawn_unit`: a possessed hero the detection channel can tell.
    fn spawn_embodied(world: &mut World, faction: Faction, pos: Vec2) -> Entity {
        let e = spawn_unit(world, faction, pos);
        world.input_source[e.index as usize] = InputSource::Embodied;
        e
    }

    /// A scene where an idle Enemy unit (which doubles as the detection observer) sits in plain,
    /// in-range sight of a gone-dark Player hero, with a neutral point as the baseline objective.
    /// Returns `(world, terrain, territory, hero_pos, point_pos)`.
    fn hunt_scene() -> (World, Terrain, Territory, Vec2, Vec2) {
        let mut world = World::new();
        // The Enemy unit at the origin is BOTH the unit we task AND the faction's detection observer.
        spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        let hero_pos = at(5, 0); // within the default tell_range (28), open LoS → detectable
        spawn_embodied(&mut world, Faction::Player, hero_pos);
        let point_pos = at(10, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(point_pos)],
        };
        (world, Terrain::open(), terr, hero_pos, point_pos)
    }

    /// Derive the detection channel exactly as the host would, for `observer` over `world`/`terrain`.
    fn tells_for(world: &World, terrain: &Terrain, mode: TellMode, observer: Faction) -> Vec<Tell> {
        let config = DetectionConfig {
            tell_mode: mode,
            ..DetectionConfig::default()
        };
        let mut mem = DetectionMemory::new();
        detectable_embodiment(world, terrain, &config, observer, 0, &mut mem)
    }

    /// 1. **Default-off → byte-identical.** With `hunt_embodied = false`, the commander emits the
    ///    EXACT same command list whether or not detection tells are supplied — the gone-dark code
    ///    is fully bypassed, so the default scenes' lockstep/checksum streams are untouched.
    #[test]
    fn hunt_disabled_is_byte_identical_regardless_of_tells() {
        let (world, terrain, terr, _hero, _point) = hunt_scene();
        let res = Resources::new(0); // no production noise
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(!tells.is_empty(), "scene precondition: the hero IS detectable");

        let mut rng = Rng::new(7);
        let baseline = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            60,
        );
        // Same default (off) config, but now WITH a live tell present: must be ignored entirely.
        let mut rng = Rng::new(7);
        let with_tells_off = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig {
                hunt_embodied: false,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            60,
        );
        assert_eq!(
            baseline.len(),
            with_tells_off.len(),
            "flag off must ignore tells → identical command count"
        );
        for (x, y) in baseline.iter().zip(with_tells_off.iter()) {
            assert_eq!(
                format!("{x:?}"),
                format!("{y:?}"),
                "flag off must emit a byte-identical command stream even with tells present"
            );
        }
        // And the baseline genuinely heads for the capture point (so this test has real teeth).
        assert!(
            baseline
                .iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == _point)),
            "baseline should capture the open point: {baseline:?}"
        );
    }

    /// 2. **Enabled → reacts.** With `hunt_embodied = true` and a detectable gone-dark hostile, a
    ///    free unit is pressed toward the hero's revealed position INSTEAD of the capture point — a
    ///    different, sensible (honest) order responding to the tell.
    #[test]
    fn hunt_enabled_chases_detectable_gone_dark_hostile() {
        let (world, terrain, terr, hero, point) = hunt_scene();
        let res = Resources::new(0);
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(!tells.is_empty(), "scene precondition: the hero IS detectable");

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == hero)),
            "the commander should press toward the gone-dark hero at {hero:?}: {cmds:?}"
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "chasing the hero outranks capturing the point: {cmds:?}"
        );
    }

    /// 3a. **Honest bound — out of range.** Flag ON, but the hero is beyond `tell_range`, so the
    ///     detection channel reveals NOTHING (empty tells) and the commander does NOT react — it
    ///     falls back to the ordinary capture plan. No omniscient peek.
    #[test]
    fn hunt_does_not_react_when_hostile_out_of_detection_range() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(0, 0)); // observer + the unit we task
        spawn_embodied(&mut world, Faction::Player, at(60, 0)); // far beyond default tell_range 28
        let point = at(10, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(point)],
        };
        let terrain = Terrain::open();
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(
            tells.is_empty(),
            "out of range → detection legitimately reveals nothing"
        );

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "with no tell, the commander reverts to capturing the point: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(
                |c| matches!(c, Command::AttackMove { target, .. } if *target == at(60, 0))
            ),
            "the commander must NOT know the secret hero position (no omniscient peek): {cmds:?}"
        );
    }

    /// 3b. **Honest bound — line of sight blocked.** Flag ON and in range, but a wall blocks LoS, so
    ///     the channel reveals nothing and the commander does not react.
    #[test]
    fn hunt_does_not_react_when_line_of_sight_blocked() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        spawn_embodied(&mut world, Faction::Player, at(10, 0)); // in range, but...
        let mut terrain = Terrain::open();
        terrain.set_cover(69, 64, Cover::Heavy); // ...a wall strictly between (cells 64↔74)
        assert!(!terrain.line_of_sight(at(0, 0), at(10, 0)));
        let point = at(0, 12); // well outside the commit radius (6) so it IS a capture target
        let terr = Territory {
            points: vec![ControlPoint::neutral(point)],
        };
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(tells.is_empty(), "no LoS → detection reveals nothing");

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "LoS-blocked → no reaction, ordinary capture plan: {cmds:?}"
        );
    }

    /// 3c. **Honest bound — `Hidden` mode.** Even point-blank in plain sight, `TellMode::Hidden`
    ///     yields no tells, so a commander that consults the channel gains ZERO knowledge — the
    ///     "no omniscient peek" property is structural, not a discipline.
    #[test]
    fn hunt_gains_nothing_in_hidden_tell_mode() {
        let (world, terrain, terr, _hero, point) = hunt_scene(); // hero in plain, in-range sight
        let tells = tells_for(&world, &terrain, TellMode::Hidden, Faction::Enemy);
        assert!(tells.is_empty(), "Hidden mode reveals nothing, ever");

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "Hidden mode → the commander chases nothing, captures as usual: {cmds:?}"
        );
    }

    /// 4. **Deterministic.** Identical inputs (world, tells, config, seed, tick) ⇒ identical command
    ///    list, twice over — the gone-dark path adds no float and no nondeterminism.
    #[test]
    fn hunt_is_deterministic_for_identical_inputs() {
        let (world, terrain, terr, _hero, _point) = hunt_scene();
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        let cfg = CommanderConfig {
            hunt_embodied: true,
            ..CommanderConfig::default()
        };
        let run = || {
            let mut rng = Rng::new(99);
            commander_orders(&world, &terr, &Resources::new(0), &mut rng, &cfg, &tells, Faction::Enemy, 60)
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), b.len(), "same inputs → same command count");
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(format!("{x:?}"), format!("{y:?}"), "hunt command stream diverged");
        }
    }

    /// The tell picker mirrors the other "nearest" scans: nearest by squared distance, stable
    /// tie-break toward the earliest tell in the slice. (No sqrt, no float.)
    #[test]
    fn nearest_tell_picks_nearest_with_stable_tiebreak() {
        let dummy = World::new().spawn(); // an entity handle; only `pos` matters to the picker
        let t = |x: i32, y: i32| Tell {
            unit: dummy,
            pos: at(x, y),
            age_ticks: 0,
        };
        // Strictly nearer wins regardless of order.
        let tells = [t(40, 0), t(5, 0)];
        assert_eq!(nearest_tell(&tells, at(0, 0)), Some(at(5, 0)));
        // Equal distance → the earlier slice entry wins (stable).
        let tied = [t(10, 0), t(-10, 0)];
        assert_eq!(nearest_tell(&tied, at(0, 0)), Some(at(10, 0)));
        // Empty slice → nothing.
        assert_eq!(nearest_tell(&[], at(0, 0)), None);
    }

    // --- Difficulty tiers (WS-E) -------------------------------------------------------------
    //
    // A tier scales the SEEDED planner's *choices* — production backlog depth, the Heavy reserve,
    // and the army re-plan cadence — never its *knowledge*. None of this reads the player's
    // embodiment/fog state (invariant #6 / §9): the gone-dark consult stays gated solely by
    // `hunt_embodied`, independent of tier. So a (mission, tier, seed) replays bit-identically,
    // and harder tiers are a *better commander*, not an omniscient one.

    use crate::components::ProductionItem;
    use crate::mission_tuning::Difficulty;

    /// A config at an explicit tier, hunt off (the difficulty axis in isolation).
    fn tier_cfg(difficulty: Difficulty) -> CommanderConfig {
        CommanderConfig {
            difficulty,
            ..CommanderConfig::default()
        }
    }

    /// The default config is the `Veteran` tier — and produces a byte-identical command stream to
    /// an explicitly-`Veteran` config. This is the property that keeps the default scenes' golden
    /// checksums untouched after the difficulty knob was added.
    #[test]
    fn default_config_is_veteran_and_byte_identical() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        spawn_built_camp(&mut world, Faction::Enemy, at(30, 0));
        spawn_unit(&mut world, Faction::Player, at(-5, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let res = Resources::new(500);

        assert_eq!(CommanderConfig::default().difficulty, Difficulty::Veteran);

        let run = |cfg: &CommanderConfig| {
            let mut rng = Rng::new(42);
            commander_orders(&world, &terr, &res, &mut rng, cfg, &[], Faction::Enemy, 0)
        };
        let default = run(&CommanderConfig::default());
        let veteran = run(&tier_cfg(Difficulty::Veteran));
        assert_eq!(default.len(), veteran.len());
        for (x, y) in default.iter().zip(veteran.iter()) {
            assert_eq!(format!("{x:?}"), format!("{y:?}"), "default must equal explicit Veteran");
        }
    }

    /// **Reserve / unit-mix knob.** At a purse that sits between the two thresholds, `Elite` (zero
    /// reserve) splurges on a Heavy while `Veteran` (a one-Rifleman reserve) buys the cheap body —
    /// the same honest survey, different spending discipline.
    #[test]
    fn elite_buys_heavy_where_veteran_buys_rifleman() {
        let mut world = World::new();
        let camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        let terr = Territory::empty();
        // 250: above HEAVY_COST (220) but below Veteran's HEAVY_COST + one Rifleman (320).
        let purse = economy::HEAVY_COST + 30;

        let queued = |cfg: &CommanderConfig| -> Vec<UnitKind> {
            let mut rng = Rng::new(1);
            commander_orders(&world, &terr, &Resources::new(purse), &mut rng, cfg, &[], Faction::Enemy, 0)
                .into_iter()
                .filter_map(|c| match c {
                    Command::QueueProduction { camp: cc, unit } if cc == camp => Some(unit),
                    _ => None,
                })
                .collect()
        };

        assert_eq!(
            queued(&tier_cfg(Difficulty::Veteran)),
            vec![UnitKind::Rifleman],
            "Veteran keeps a reserve → cheap body at this purse"
        );
        assert_eq!(
            queued(&tier_cfg(Difficulty::Elite)),
            vec![UnitKind::Heavy],
            "Elite keeps no reserve → splurges on the Heavy the moment it's affordable"
        );
    }

    /// **Aggression knob.** With the camp already holding one queued item, whether the commander
    /// stacks a *second* depends on the tier's `max_queue_depth`: `Recruit` (1) declines, `Veteran`
    /// (2) and `Elite` (3) add one. A pure, single-call read of the backlog cap.
    #[test]
    fn backlog_depth_scales_with_tier() {
        let queues_more = |difficulty: Difficulty| -> bool {
            let mut world = World::new();
            let _camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
            // Pre-load one item so the depth check is what decides a second.
            world.building[_camp.index as usize].queue.push(ProductionItem {
                kind: UnitKind::Rifleman,
                ticks_left: 10,
            });
            let terr = Territory::empty();
            let mut rng = Rng::new(1);
            commander_orders(
                &world,
                &terr,
                &Resources::new(10_000), // flush, so only the depth cap gates
                &mut rng,
                &tier_cfg(difficulty),
                &[],
                Faction::Enemy,
                0,
            )
            .iter()
            .any(|c| matches!(c, Command::QueueProduction { .. }))
        };
        assert!(!queues_more(Difficulty::Recruit), "Recruit (depth 1) won't stack a second");
        assert!(queues_more(Difficulty::Veteran), "Veteran (depth 2) stacks a second");
        assert!(queues_more(Difficulty::Elite), "Elite (depth 3) stacks a second");
    }

    /// **Cadence knob.** `Recruit` (stride 2) re-tasks its army only on even commander cycles, so on
    /// an off-cycle it issues no movement order even with an idle unit and an open point; `Veteran`
    /// (stride 1) re-tasks every cycle. Reinforcement is unaffected (not strided). Pure function of
    /// `tick`, so it stays deterministic across peers.
    #[test]
    fn cadence_stride_skips_retask_on_off_cycle() {
        // An idle Enemy unit with a neutral point to take → a re-task cycle yields one AttackMove.
        let scene = || {
            let mut world = World::new();
            spawn_unit(&mut world, Faction::Enemy, at(20, 0));
            let terr = Territory {
                points: vec![ControlPoint::neutral(at(5, 0))],
            };
            (world, terr)
        };
        let has_attackmove = |difficulty: Difficulty, tick: u64| -> bool {
            let (world, terr) = scene();
            let mut rng = Rng::new(1);
            commander_orders(
                &world,
                &terr,
                &Resources::new(0), // no production noise
                &mut rng,
                &tier_cfg(difficulty),
                &[],
                Faction::Enemy,
                tick,
            )
            .iter()
            .any(|c| matches!(c, Command::AttackMove { .. }))
        };

        // Cycle 0 (tick 0): both tiers re-task.
        assert!(has_attackmove(Difficulty::Recruit, 0), "on-cycle: Recruit re-tasks");
        assert!(has_attackmove(Difficulty::Veteran, 0), "Veteran always re-tasks");
        // Cycle 1 (tick = one period): Recruit skips (stride 2), Veteran still acts (stride 1).
        assert!(
            !has_attackmove(Difficulty::Recruit, COMMANDER_PERIOD),
            "off-cycle: Recruit's sluggish cadence skips the re-task"
        );
        assert!(
            has_attackmove(Difficulty::Veteran, COMMANDER_PERIOD),
            "Veteran re-tasks every cycle regardless"
        );
    }

    /// The headline WS-E property: a given **mission + tier + seed** replays **bit-identically**.
    /// The commander draws from its own stream seeded `sim_seed ^ faction` (never `Sim::rng`); two
    /// runs at the same tier produce the identical command stream, and the tier genuinely changes
    /// the plan (so the knob has teeth) — all without any float or omniscient read.
    #[test]
    fn mission_tier_seed_replays_bit_identically() {
        const SIM_SEED: u64 = 0xD0E1;
        let scene = || {
            let mut world = World::new();
            spawn_unit(&mut world, Faction::Enemy, at(20, 0));
            spawn_unit(&mut world, Faction::Enemy, at(22, 3));
            spawn_built_camp(&mut world, Faction::Enemy, at(30, 0));
            spawn_unit(&mut world, Faction::Player, at(-5, 0));
            let terr = Territory {
                points: vec![ControlPoint::neutral(at(0, 0))],
            };
            (world, terr)
        };
        // The commander RNG is the host's own stream, seeded sim_seed ^ faction.
        let plan = |difficulty: Difficulty| -> Vec<String> {
            let (world, terr) = scene();
            let mut rng = Rng::new(SIM_SEED ^ Faction::Enemy.index() as u64);
            commander_orders(
                &world,
                &terr,
                // A purse that makes the unit-mix knob observable (250).
                &Resources::new(economy::HEAVY_COST + 30),
                &mut rng,
                &tier_cfg(difficulty),
                &[],
                Faction::Enemy,
                0,
            )
            .iter()
            .map(|c| format!("{c:?}"))
            .collect()
        };

        for d in Difficulty::ALL {
            assert_eq!(plan(d), plan(d), "same (mission, tier, seed) ⇒ identical stream");
        }
        // ...and distinct tiers really do reshape the plan (Veteran rifle vs Elite heavy here).
        assert_ne!(
            plan(Difficulty::Veteran),
            plan(Difficulty::Elite),
            "the difficulty knob must change the command stream"
        );
    }
}

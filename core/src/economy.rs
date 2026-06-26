//! Economy, camps, and production (invariant #1 — integer/fixed-point, deterministic).
//!
//! Holds the per-faction [`Resources`] purse and drives buildings: construction progress,
//! upgrades, territory-fed income, and FIFO unit production that spawns finished units into
//! the world. All command entry points ([`build`], [`upgrade`], [`queue_production`]) are
//! pure functions the sim calls from `Sim::apply`; the per-tick advance is [`economy_system`].
//!
//! Determinism: resource counts are plain `i64` (no float money), income/build/production all
//! advance by integer ticks in stable entity-index order, and produced units get their stats
//! from a fixed [`UnitKind`] table so every peer spawns the bit-identical unit.
//!
//! KEEP the `Resources` field shape (`amounts: [i64; FACTION_COUNT]`) and all public signatures
//! intact — the sim's checksum folds `Resources` by that shape.

use crate::components::{
    Building, BuildingKind, EntityKind, Faction, Health, Order, ProductionItem, Stance, UnitKind,
    Vec2, Weapon, FACTION_COUNT,
};
use crate::ecs::{Entity, World};
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::rng::Rng;
use crate::territory::Territory;

// ===========================================================================
// Cost / time / stat tables. All integer or fixed-point (invariant #1). These
// are the single source of truth every peer reads, so the same action costs and
// the same unit spawns identically everywhere (lockstep).
//
// MEASURED BALANCE BASELINE (D30 — supersedes the D26 first pass).
// ---------------------------------------------------------------------------
// Still a *playtest baseline*, NOT a locked design — but every combat/cost
// number here was moved against an objective, deterministic metric (the
// sim-runner `--metrics` harness: open 1v1 time-to-kill, equal-cost army
// trades, suppression pin-vs-kill timing, the economy ramp curve), so the
// shape is justified by measurement rather than vibe. Final *feel* still
// awaits human playtests. Reasoned in *seconds* (the sim runs at 60 Hz, so
// `seconds * 60 = ticks`) and against the demo's seed purse of
// `Resources::new(500)` (see sim-runner / engine). The economy must remain
// integer/fixed-point (invariant #1) and bit-identical dev==release.
//
// The shape of the design:
//   * Income drips per-tick. With base 1/tick that is 60 resources/second of
//     hands-off income; each held point adds 2/tick = 120/second. So holding
//     one point roughly *triples* your income — territory genuinely matters.
//   * Costs are sized in that 60/s frame so they read in seconds of saving:
//     a Rifleman (~1.7 s of base income) is cheap, spammable, long-ranged; a
//     Heavy (~3.7 s) is a real investment that buys 2.8x HP and 3x burst at
//     SHORTER range — a short-range bruiser, not a strict upgrade; a camp
//     (~4 s, half the seed purse) is a commitment but affordable turn-one.
//   * The Rifleman↔Heavy matchup is range-dependent by design: at point-blank
//     the cost-equal Heavy blob out-trades the rifles; at rifle range the
//     cheaper, longer-reaching rifles kite and win (harness-verified — the old
//     Heavy was strictly dominated and lost every equal-cost trade).
//   * Build/production times read in seconds: Rifleman a handful of seconds,
//     Heavy notably longer, camp construction longer still.
//   * A camp + one held point pays its 250 cost back in ~2 s of holding, so
//     "expand + bank a camp" is a real economic line against "spend it on
//     bodies now" — that fork is the intended decision.
// ===========================================================================

/// Cost (resources) to start building a [`Camp`](BuildingKind::Camp).
/// 250 = half the demo seed purse (500): a genuine commitment, yet you can
/// still afford exactly one turn-one (leaving 250 for an opening unit or two).
pub const CAMP_BUILD_COST: i64 = 250;

/// Resource cost to produce one [`Rifleman`](UnitKind::Rifleman).
/// 100 ≈ 1.7 s of base income (60/s): cheap and spammable, the bread-and-butter
/// body you mass.
pub const RIFLEMAN_COST: i64 = 100;
/// Resource cost to produce one [`Heavy`](UnitKind::Heavy).
/// 220 = 2.2x a Rifleman. The Heavy is a short-range *bruiser*: 2.8x HP (280 vs 100)
/// and 3x burst (18 vs 6 dmg) at *shorter* range (11 vs 14). The 2.2x cost is tuned
/// (D30) so the equal-cost trade is genuinely range-dependent — at point-blank the
/// Heavy mass trades up, at rifle range the cheaper, longer-reaching Rifleman mass
/// wins — instead of the old strictly-dominated Heavy (measured rifle-mass-wipes-
/// heavy under D26's numbers).
pub const HEAVY_COST: i64 = 220;

/// Ticks to finish a freshly-placed camp's construction. 1200 ticks = 20 s — a
/// camp is a slow, deliberate structural commitment, far longer than any unit.
pub const CAMP_BUILD_TICKS: u16 = 1200;

/// Base ticks to produce one [`Rifleman`](UnitKind::Rifleman) (before level
/// speedup). 300 ticks = 5 s: a handful of seconds, fast enough to reinforce.
pub const RIFLEMAN_BASE_TICKS: u16 = 300;
/// Base ticks to produce one [`Heavy`](UnitKind::Heavy) (before level speedup).
/// 660 ticks = 11 s: notably longer than a Rifleman, matching its higher cost
/// (2.2x) and battlefield value. Kept proportional to the 220 cost (3x the Rifleman
/// production time for 2.2x the cost) so producing Heavies stays a deliberate, slow
/// commitment rather than a spam option (D30).
pub const HEAVY_BASE_TICKS: u16 = 660;

/// Each upgrade level shaves this many ticks off production time...
/// 60 ticks = 1 s faster per level — a tangible, readable reward for investing
/// in a camp instead of (or alongside) more bodies.
pub const LEVEL_PROD_SPEEDUP: u16 = 60;
/// ...down to no faster than this floor (so a maxed camp can't produce instantly).
/// 120 ticks = 2 s: even a fully-upgraded camp still takes a beat per unit, so
/// production speed never trivializes the army-vs-economy tension.
pub const PROD_TICKS_FLOOR: u16 = 120;

/// Resources every faction accrues per tick regardless of held territory.
/// 1/tick = 60/second — a steady hands-off drip you always get.
pub const BASE_INCOME: i64 = 1;
/// Extra per-tick resources per controlled territory point.
/// 2/tick = 120/second per point — each point roughly *doubles* base income, so
/// holding ground is the dominant way to out-produce an opponent.
pub const PER_POINT_INCOME: i64 = 2;

/// Starting HP of a [`Camp`](BuildingKind::Camp). 1000 HP — ~4.5x a Rifleman and
/// ~4.5x a Heavy: a strategic structure that takes a sustained commitment to
/// raze, not something a stray squad deletes in passing.
const CAMP_HP: i32 = 1000;

/// Cost to upgrade a camp currently at `level` to the next tier: `200 * (level + 1)`.
/// Level 0→1 costs 200 (≈ two Riflemen), and each tier costs more (200, 400,
/// 600, …) so deep upgrades are a real resource sink competing with army size.
#[inline]
pub const fn upgrade_cost(level: u8) -> i64 {
    200 * (level as i64 + 1)
}

/// Resource cost to produce one unit of `kind`.
#[inline]
pub const fn unit_cost(kind: UnitKind) -> i64 {
    match kind {
        UnitKind::Rifleman => RIFLEMAN_COST,
        UnitKind::Heavy => HEAVY_COST,
    }
}

/// Production time (ticks) for `kind` at a camp of `level`. Higher tiers produce faster,
/// clamped to [`PROD_TICKS_FLOOR`] so production is always at least that many ticks.
#[inline]
pub const fn prod_time(kind: UnitKind, level: u8) -> u16 {
    let base = match kind {
        UnitKind::Rifleman => RIFLEMAN_BASE_TICKS,
        UnitKind::Heavy => HEAVY_BASE_TICKS,
    };
    let speedup = LEVEL_PROD_SPEEDUP.saturating_mul(level as u16);
    let reduced = base.saturating_sub(speedup);
    if reduced < PROD_TICKS_FLOOR {
        PROD_TICKS_FLOOR
    } else {
        reduced
    }
}

/// Build cost for a `kind` building.
#[inline]
pub const fn build_cost(kind: BuildingKind) -> i64 {
    match kind {
        BuildingKind::Camp => CAMP_BUILD_COST,
    }
}

/// Fixed combat stats a produced unit spawns with — looked up from [`UnitKind`] so every peer
/// spawns the bit-identical unit (determinism).
pub fn unit_stats(kind: UnitKind) -> (Health, Weapon) {
    match kind {
        // Measured re-tune (D30 — supersedes the D24/D26 first pass). Numbers chosen against the
        // deterministic balance harness (sim-runner `--metrics`), not feel:
        //   * Rifleman: 6 dmg / 30 ticks = 12 DPS → a symmetric open 1v1 resolves in ~8.0 s
        //     (down from the old ~12 s — combat is decisive without being a contact-delete), and
        //     long-reaching (range 14) so rifle MASS wins at range.
        //   * Heavy: a short-range BRUISER — 280 HP, 18 dmg / 48 ticks (~22.5 DPS) at range 11.
        //     It out-trades a cost-equal Rifleman blob at point-blank but is kited by the longer-
        //     ranged Rifleman, so neither strictly dominates (the old 220/11/range-12 Heavy lost
        //     every equal-cost trade; harness confirmed rifle-mass wiped heavy-mass 0-for).
        // These remain a *playtest baseline* (measured targets, not final feel — D30 keeps D26's
        // "baseline, not locked" framing); dial against fresh `--metrics` runs.
        UnitKind::Rifleman => (
            Health::full(Fixed::from_int(100)),
            Weapon {
                range: Fixed::from_int(14),
                damage: Fixed::from_int(6),
                cooldown_ticks: 30,
                cooldown_left: 0,
                // Magazine gates only the embodied fire path (auto-combat ignores it — see
                // `Weapon`/`combat::resolve_fire`): a 30-round mag, 90-tick reload (≈1500 ms
                // at 60 Hz).
                mag_size: 30,
                ammo: 30,
                reload_ticks: 90,
                reload_left: 0,
                // Infantry rifle: fixed mount, no independent turret (P2 default).
                turret_speed: 0,
                // Hitscan infantry weapon (P3 default): no shell flight, resolves instantly.
                muzzle_vel: Fixed::ZERO,
                // No armour penetration (P4 default): full damage vs the unarmoured units it fights
                // (multiplier 1.0); only bites against a future armoured kind. Balance unchanged.
                penetration: Fixed::ZERO,
            },
        ),
        UnitKind::Heavy => (
            Health::full(Fixed::from_int(280)),
            Weapon {
                range: Fixed::from_int(11),
                damage: Fixed::from_int(18),
                cooldown_ticks: 48,
                cooldown_left: 0,
                // Bigger belt, slower 138-tick reload (≈2300 ms) — the bruiser sustains fire
                // longer but is punished harder for running dry while embodied.
                mag_size: 50,
                ammo: 50,
                reload_ticks: 138,
                reload_left: 0,
                // Heavy infantry bruiser: still a fixed mount (the playable tank is the new
                // dedicated kind, plan §3). No independent turret here.
                turret_speed: 0,
                // Hitscan infantry weapon (P3 default): no shell flight, resolves instantly.
                muzzle_vel: Fixed::ZERO,
                // No armour penetration (P4 default) — unchanged balance vs unarmoured units.
                penetration: Fixed::ZERO,
            },
        ),
    }
}

/// Per-faction resource purse. Indexed by [`Faction::index`]; plain `i64` so there is no
/// float money in the deterministic sim. SHAPE IS PINNED (checksum folds `amounts`).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Resources {
    pub amounts: [i64; FACTION_COUNT],
}

impl Resources {
    /// Start every faction with `initial` resources.
    pub fn new(initial: i64) -> Self {
        Resources {
            amounts: [initial; FACTION_COUNT],
        }
    }

    #[inline]
    pub fn get(&self, faction: Faction) -> i64 {
        self.amounts[faction.index()]
    }

    #[inline]
    pub fn add(&mut self, faction: Faction, delta: i64) {
        self.amounts[faction.index()] += delta;
    }

    /// Spend `cost` if affordable; returns whether the spend happened (no debt allowed).
    #[inline]
    pub fn try_spend(&mut self, faction: Faction, cost: i64) -> bool {
        let i = faction.index();
        if self.amounts[i] >= cost {
            self.amounts[i] -= cost;
            true
        } else {
            false
        }
    }
}

/// Start construction of a `kind` building for `faction` at `pos`, spending its cost. Returns
/// the new building entity, or `None` if unaffordable. STUB (worker 3).
pub fn build(
    world: &mut World,
    resources: &mut Resources,
    faction: Faction,
    kind: BuildingKind,
    pos: Vec2,
) -> Option<Entity> {
    if !resources.try_spend(faction, build_cost(kind)) {
        return None;
    }
    let e = world.spawn();
    let i = e.index as usize;
    world.kind[i] = EntityKind::Building;
    world.faction[i] = faction;
    world.pos[i] = pos;
    world.health[i] = Health::full(Fixed::from_int(CAMP_HP));
    world.order[i] = Order::Idle;
    world.building[i] = Building {
        kind,
        level: 0,
        build_ticks_left: CAMP_BUILD_TICKS,
        queue: Vec::new(),
    };
    Some(e)
}

/// Upgrade a built camp one level, spending the upgrade cost. Returns whether it happened.
/// STUB (worker 3).
pub fn upgrade(world: &mut World, resources: &mut Resources, camp: Entity) -> bool {
    if !world.is_alive(camp) {
        return false;
    }
    let i = camp.index as usize;
    if world.kind[i] != EntityKind::Building || world.building[i].build_ticks_left != 0 {
        return false;
    }
    let level = world.building[i].level;
    if !resources.try_spend(world.faction[i], upgrade_cost(level)) {
        return false;
    }
    world.building[i].level = level.saturating_add(1);
    true
}

/// Enqueue a `unit` for production at a built `camp`, spending its cost. Returns whether it
/// was queued. STUB (worker 3).
pub fn queue_production(
    world: &mut World,
    resources: &mut Resources,
    camp: Entity,
    unit: UnitKind,
) -> bool {
    if !world.is_alive(camp) {
        return false;
    }
    let i = camp.index as usize;
    if world.kind[i] != EntityKind::Building || world.building[i].build_ticks_left != 0 {
        return false;
    }
    if !resources.try_spend(world.faction[i], unit_cost(unit)) {
        return false;
    }
    let level = world.building[i].level;
    world.building[i].queue.push(ProductionItem {
        kind: unit,
        ticks_left: prod_time(unit, level),
    });
    true
}

/// Advance one tick of economy: income from held territory, construction, upgrades, and
/// production (spawning finished units). STUB (worker 3) — no-op so the scaffold compiles.
pub fn economy_system(
    world: &mut World,
    resources: &mut Resources,
    territory: &Territory,
    events: &mut Vec<SimEvent>,
    rng: &mut Rng,
) {
    let _ = rng;

    // --- INCOME (per-tick integer accrual; Neutral never earns) ---
    for &faction in Faction::ALL.iter() {
        if faction == Faction::Neutral {
            continue;
        }
        let count = territory.controlled_count(faction) as i64;
        resources.add(faction, BASE_INCOME + PER_POINT_INCOME * count);
    }

    // --- BUILDINGS: construction + production countdown ---
    // First scan (index order): advance construction, count down the front production item,
    // and record any camp whose front item completed THIS tick. We do not spawn here —
    // `world.spawn()` may reallocate the SoA Vecs, so we collect completions and spawn after,
    // still in index order (deterministic).
    let mut completed: Vec<(usize, UnitKind)> = Vec::new();
    let cap = world.capacity();
    for i in 0..cap {
        if !world.is_index_alive(i) || world.kind[i] != EntityKind::Building {
            continue;
        }
        if world.building[i].build_ticks_left > 0 {
            world.building[i].build_ticks_left -= 1;
            continue;
        }
        if world.building[i].kind != BuildingKind::Camp {
            continue;
        }
        if let Some(front) = world.building[i].queue.first_mut() {
            if front.ticks_left > 0 {
                front.ticks_left -= 1;
            }
            if front.ticks_left == 0 {
                let item = world.building[i].queue.remove(0);
                completed.push((i, item.kind));
            }
        }
    }

    // Second pass: spawn finished units (index order preserved).
    for (camp_i, unit_kind) in completed {
        let faction = world.faction[camp_i];
        let pos = world.pos[camp_i];
        let (health, weapon) = unit_stats(unit_kind);
        let e = world.spawn();
        let ei = e.index as usize;
        world.kind[ei] = EntityKind::Unit;
        world.unit_kind[ei] = unit_kind;
        world.faction[ei] = faction;
        world.pos[ei] = pos;
        world.health[ei] = health;
        world.weapon[ei] = weapon;
        world.order[ei] = Order::Idle;
        world.stance[ei] = Stance::ReturnFire;
        events.push(SimEvent::UnitProduced { faction, pos });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::territory::ControlPoint;

    fn empty_terr() -> Territory {
        Territory::empty()
    }

    fn tick(world: &mut World, res: &mut Resources, terr: &Territory) -> Vec<SimEvent> {
        let mut events = Vec::new();
        let mut rng = Rng::new(1);
        economy_system(world, res, terr, &mut events, &mut rng);
        events
    }

    fn alive_units(world: &World, faction: Faction) -> usize {
        let mut n = 0;
        for i in 0..world.capacity() {
            if world.is_index_alive(i)
                && world.kind[i] == EntityKind::Unit
                && world.faction[i] == faction
            {
                n += 1;
            }
        }
        n
    }

    #[test]
    fn try_spend_rejects_when_poor_and_debits_when_affordable() {
        let mut res = Resources::new(40);
        assert!(!res.try_spend(Faction::Player, 50));
        assert_eq!(
            res.get(Faction::Player),
            40,
            "rejected spend must not debit"
        );
        assert!(res.try_spend(Faction::Player, 30));
        assert_eq!(res.get(Faction::Player), 10);
        // Exact-balance spend succeeds.
        assert!(res.try_spend(Faction::Player, 10));
        assert_eq!(res.get(Faction::Player), 0);
    }

    #[test]
    fn build_creates_under_construction_building_and_debits() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST);
        let e = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .expect("affordable build should return Some");
        let i = e.index as usize;
        assert_eq!(res.get(Faction::Player), 0, "build must debit cost");
        assert_eq!(world.kind[i], EntityKind::Building);
        assert_eq!(world.faction[i], Faction::Player);
        assert_eq!(world.building[i].build_ticks_left, CAMP_BUILD_TICKS);
        assert_eq!(world.building[i].level, 0);
        assert!(world.building[i].queue.is_empty());
        assert_eq!(world.health[i], Health::full(Fixed::from_int(CAMP_HP)));
    }

    #[test]
    fn build_too_poor_returns_none_and_does_not_debit() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST - 1);
        let r = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        );
        assert!(r.is_none());
        assert_eq!(res.get(Faction::Player), CAMP_BUILD_COST - 1);
        assert_eq!(world.capacity(), 0, "no entity should have spawned");
    }

    #[test]
    fn economy_system_ticks_construction_to_built() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST);
        let e = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let i = e.index as usize;
        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            assert!(world.building[i].build_ticks_left > 0);
            tick(&mut world, &mut res, &terr);
        }
        assert_eq!(
            world.building[i].build_ticks_left, 0,
            "camp should be built after CAMP_BUILD_TICKS ticks"
        );
    }

    #[test]
    fn queue_production_then_run_spawns_one_unit_and_debits() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST + RIFLEMAN_COST);
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let terr = empty_terr();

        // Finish construction (income would distort balances, so use empty territory and
        // measure against the income we know we accrue).
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }
        let before = res.get(Faction::Player);
        assert!(queue_production(
            &mut world,
            &mut res,
            camp,
            UnitKind::Rifleman
        ));
        assert_eq!(
            res.get(Faction::Player),
            before - RIFLEMAN_COST,
            "queueing must debit the unit cost"
        );

        assert_eq!(alive_units(&world, Faction::Player), 0);
        let ptime = prod_time(UnitKind::Rifleman, 0);
        let mut produced_events = 0;
        for _ in 0..ptime {
            let evs = tick(&mut world, &mut res, &terr);
            produced_events += evs
                .iter()
                .filter(|e| matches!(e, SimEvent::UnitProduced { .. }))
                .count();
        }
        assert_eq!(alive_units(&world, Faction::Player), 1, "exactly one unit");
        assert_eq!(produced_events, 1, "exactly one UnitProduced event");

        // Verify the spawned unit's stats.
        let mut found = false;
        for i in 0..world.capacity() {
            if world.is_index_alive(i) && world.kind[i] == EntityKind::Unit {
                let (h, w) = unit_stats(UnitKind::Rifleman);
                assert_eq!(world.faction[i], Faction::Player);
                assert_eq!(world.health[i], h);
                assert_eq!(world.weapon[i], w);
                assert_eq!(world.stance[i], Stance::ReturnFire);
                assert_eq!(world.order[i], Order::Idle);
                found = true;
            }
        }
        assert!(found);
    }

    #[test]
    fn production_spawns_unit_with_its_queued_kind() {
        // The load-bearing render-metadata seam: a Heavy queued through production must spawn
        // carrying `UnitKind::Heavy`, a Rifleman `UnitKind::Rifleman`. Set deterministically from
        // the queue item, so it is identical on every peer (it is NOT in the checksum).
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST + RIFLEMAN_COST + HEAVY_COST);
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }

        // Produce a Rifleman, then verify the single spawned unit carries Rifleman.
        assert!(queue_production(&mut world, &mut res, camp, UnitKind::Rifleman));
        for _ in 0..prod_time(UnitKind::Rifleman, 0) {
            tick(&mut world, &mut res, &terr);
        }
        let rifle_idx = (0..world.capacity())
            .find(|&i| world.is_index_alive(i) && world.kind[i] == EntityKind::Unit)
            .expect("a rifleman should have spawned");
        assert_eq!(world.unit_kind[rifle_idx], UnitKind::Rifleman);

        // Produce a Heavy, then verify the new unit carries Heavy (and the rifleman is untouched).
        assert!(queue_production(&mut world, &mut res, camp, UnitKind::Heavy));
        for _ in 0..prod_time(UnitKind::Heavy, 0) {
            tick(&mut world, &mut res, &terr);
        }
        let heavy_idx = (0..world.capacity())
            .find(|&i| {
                world.is_index_alive(i)
                    && world.kind[i] == EntityKind::Unit
                    && i != rifle_idx
            })
            .expect("a heavy should have spawned");
        assert_eq!(world.unit_kind[heavy_idx], UnitKind::Heavy);
        assert_eq!(
            world.unit_kind[rifle_idx],
            UnitKind::Rifleman,
            "spawning the heavy must not disturb the rifleman's kind"
        );
    }

    #[test]
    fn upgrade_raises_level_and_debits() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST + upgrade_cost(0));
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        let i = camp.index as usize;

        // Unbuilt camp can't upgrade.
        assert!(!upgrade(&mut world, &mut res, camp));

        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }
        // Drain income so we can test the too-poor path precisely: spend down to exactly
        // upgrade_cost(0).
        let surplus = res.get(Faction::Player) - upgrade_cost(0);
        assert!(surplus >= 0);
        res.try_spend(Faction::Player, surplus);
        assert_eq!(res.get(Faction::Player), upgrade_cost(0));

        assert!(upgrade(&mut world, &mut res, camp));
        assert_eq!(world.building[i].level, 1);
        assert_eq!(res.get(Faction::Player), 0);

        // Now too poor for the next (more expensive) upgrade.
        assert!(!upgrade(&mut world, &mut res, camp));
        assert_eq!(world.building[i].level, 1);
    }

    #[test]
    fn upgrade_fails_on_dead_or_non_building() {
        let mut world = World::new();
        let mut res = Resources::new(10_000);
        // A plain unit entity is not a building.
        let u = world.spawn();
        let ui = u.index as usize;
        world.kind[ui] = EntityKind::Unit;
        assert!(!upgrade(&mut world, &mut res, u));

        // A despawned/stale handle.
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        world.despawn(camp);
        assert!(!upgrade(&mut world, &mut res, camp));
    }

    #[test]
    fn queue_production_fails_when_unbuilt_or_poor() {
        let mut world = World::new();
        let mut res = Resources::new(CAMP_BUILD_COST);
        let camp = build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::ZERO,
        )
        .unwrap();
        // Unbuilt: cannot queue.
        assert!(!queue_production(
            &mut world,
            &mut res,
            camp,
            UnitKind::Rifleman
        ));
        let terr = empty_terr();
        for _ in 0..CAMP_BUILD_TICKS {
            tick(&mut world, &mut res, &terr);
        }
        // Built but drain resources to 0 → too poor.
        let bal = res.get(Faction::Player);
        res.try_spend(Faction::Player, bal);
        assert_eq!(res.get(Faction::Player), 0);
        assert!(!queue_production(
            &mut world,
            &mut res,
            camp,
            UnitKind::Heavy
        ));
        assert!(world.building[camp.index as usize].queue.is_empty());
    }

    #[test]
    fn income_grows_with_owned_territory() {
        let mut world = World::new();
        let mut res = Resources::new(0);
        let terr = Territory {
            points: vec![ControlPoint {
                pos: Vec2::ZERO,
                owner: Faction::Player,
                progress: Fixed::ZERO,
            }],
        };
        let n: i64 = 10;
        for _ in 0..n {
            tick(&mut world, &mut res, &terr);
        }
        let expected = (BASE_INCOME + PER_POINT_INCOME) * n;
        assert_eq!(res.get(Faction::Player), expected);
        // Enemy owns nothing → only base income.
        assert_eq!(res.get(Faction::Enemy), BASE_INCOME * n);
        // Neutral never earns.
        assert_eq!(res.get(Faction::Neutral), 0);
    }

    #[test]
    fn higher_level_camp_produces_faster_with_floor() {
        assert!(prod_time(UnitKind::Rifleman, 1) < prod_time(UnitKind::Rifleman, 0));
        // Each level shaves exactly LEVEL_PROD_SPEEDUP off the base.
        assert_eq!(
            prod_time(UnitKind::Rifleman, 1),
            RIFLEMAN_BASE_TICKS - LEVEL_PROD_SPEEDUP
        );
        assert_eq!(
            prod_time(UnitKind::Heavy, 2),
            HEAVY_BASE_TICKS - 2 * LEVEL_PROD_SPEEDUP
        );
        // Floor is respected at a very high (saturated) level.
        assert_eq!(prod_time(UnitKind::Rifleman, 255), PROD_TICKS_FLOOR);
        assert_eq!(prod_time(UnitKind::Heavy, 255), PROD_TICKS_FLOOR);
    }

    /// Anchor the measured baseline (D30) in seconds (60 Hz) so an accidental edit that
    /// breaks the intended "reads in seconds" shape trips a test. These assertions are
    /// expected to move when the numbers are next rebalanced.
    #[test]
    fn balance_baseline_reads_in_seconds() {
        const HZ: u16 = 60;
        // Camp build is the slowest action; units are a handful of seconds.
        assert_eq!(CAMP_BUILD_TICKS, 20 * HZ, "camp construction is 20 s");
        assert_eq!(RIFLEMAN_BASE_TICKS, 5 * HZ, "rifleman is 5 s");
        assert_eq!(HEAVY_BASE_TICKS, 11 * HZ, "heavy is 11 s (D30)");
        // A camp is buildable turn-one from the 500-resource demo purse, with
        // resources to spare. (Bound to locals so the check is on values, not a
        // const expression — clippy flags `assert!` on a constant condition.)
        let (camp_cost, rifle_cost, heavy_cost) = (CAMP_BUILD_COST, RIFLEMAN_COST, HEAVY_COST);
        assert!(camp_cost < 500, "camp affordable at the seed purse");
        // Holding one point ~doubles base income (territory matters).
        assert_eq!(PER_POINT_INCOME, 2 * BASE_INCOME);
        // Heavy is a real investment over the spammable Rifleman (220 vs 100 cost — D30).
        assert!(heavy_cost > rifle_cost, "heavy costs more than a rifleman");
        assert_eq!(heavy_cost, 220, "heavy costs 220 = 11/5 of a rifleman (D30)");
    }

    /// Lock the measured D30 combat stats so a stray edit that re-breaks the tuned
    /// Rifleman/Heavy relationship (TTK band, Heavy-as-bruiser) trips a test. These are
    /// the values the `--metrics` harness was tuned against; expected to move on the next
    /// measured re-tune.
    #[test]
    fn unit_stats_match_measured_d30_baseline() {
        let (rh, rw) = unit_stats(UnitKind::Rifleman);
        assert_eq!(rh, Health::full(Fixed::from_int(100)), "rifleman 100 HP");
        assert_eq!(rw.range, Fixed::from_int(14), "rifleman range 14");
        assert_eq!(rw.damage, Fixed::from_int(6), "rifleman 6 dmg");
        assert_eq!(rw.cooldown_ticks, 30, "rifleman 30-tick cooldown -> 12 DPS, ~8 s 1v1");

        let (hh, hw) = unit_stats(UnitKind::Heavy);
        assert_eq!(hh, Health::full(Fixed::from_int(280)), "heavy 280 HP (280 vs 100 rifle)");
        assert_eq!(hw.range, Fixed::from_int(11), "heavy range 11 (shorter than rifle -> kiteable)");
        assert_eq!(hw.damage, Fixed::from_int(18), "heavy 18 dmg (3x rifle burst)");
        assert_eq!(hw.cooldown_ticks, 48, "heavy 48-tick cooldown -> 18 dmg per 48 ticks");

        // The Heavy is a bruiser, not a strict upgrade: shorter range than the Rifleman is the
        // load-bearing weakness that makes the matchup range-dependent (the old Heavy was
        // strictly dominated). Guard that relationship explicitly.
        assert!(hw.range < rw.range, "heavy must out-range LESS than the rifleman");
        assert!(hh.max > rh.max, "heavy is tankier");
        assert!(hw.damage > rw.damage, "heavy hits harder per shot");

        // Magazines are armed + start full so a freshly possessed unit can fire (embodied-only
        // gate). The bruiser carries the bigger belt and the longer reload.
        assert_eq!(rw.mag_size, 30, "rifleman 30-round mag");
        assert_eq!(rw.ammo, rw.mag_size, "spawns with a full magazine");
        assert_eq!(rw.reload_ticks, 90, "rifleman 90-tick reload");
        assert_eq!(hw.mag_size, 50, "heavy 50-round belt");
        assert_eq!(hw.ammo, hw.mag_size, "spawns with a full magazine");
        assert!(hw.mag_size > rw.mag_size, "heavy sustains fire longer");
        assert!(hw.reload_ticks > rw.reload_ticks, "heavy reload is slower");
        assert_eq!(rw.reload_left, 0, "not reloading at spawn");
        assert_eq!(hw.reload_left, 0, "not reloading at spawn");
    }
}

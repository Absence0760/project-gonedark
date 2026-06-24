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
//! IMPLEMENTATION OWNER: worker 3. This is a compiling stub. Fill in the bodies + inline
//! `#[cfg(test)]` tests. KEEP the `Resources` field shape (`amounts: [i64; FACTION_COUNT]`)
//! and all public signatures intact — the sim's checksum folds `Resources` by that shape.

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
// ===========================================================================

/// Cost (resources) to start building a [`Camp`](BuildingKind::Camp).
pub const CAMP_BUILD_COST: i64 = 200;

/// Resource cost to produce one [`Rifleman`](UnitKind::Rifleman).
pub const RIFLEMAN_COST: i64 = 50;
/// Resource cost to produce one [`Heavy`](UnitKind::Heavy).
pub const HEAVY_COST: i64 = 120;

/// Ticks to finish a freshly-placed camp's construction.
pub const CAMP_BUILD_TICKS: u16 = 120;

/// Base ticks to produce one [`Rifleman`](UnitKind::Rifleman) (before level speedup).
pub const RIFLEMAN_BASE_TICKS: u16 = 90;
/// Base ticks to produce one [`Heavy`](UnitKind::Heavy) (before level speedup).
pub const HEAVY_BASE_TICKS: u16 = 180;

/// Each upgrade level shaves this many ticks off production time...
pub const LEVEL_PROD_SPEEDUP: u16 = 15;
/// ...down to no faster than this floor (so a maxed camp can't produce instantly).
pub const PROD_TICKS_FLOOR: u16 = 30;

/// Resources every faction accrues per tick regardless of held territory.
pub const BASE_INCOME: i64 = 1;
/// Extra per-tick resources per controlled territory point.
pub const PER_POINT_INCOME: i64 = 2;

/// Starting HP of a [`Camp`](BuildingKind::Camp).
const CAMP_HP: i32 = 600;

/// Cost to upgrade a camp currently at `level` to the next tier: `150 * (level + 1)`.
#[inline]
pub const fn upgrade_cost(level: u8) -> i64 {
    150 * (level as i64 + 1)
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
        UnitKind::Rifleman => (
            Health::full(Fixed::from_int(100)),
            Weapon {
                range: Fixed::from_int(15),
                damage: Fixed::from_int(8),
                cooldown_ticks: 30,
                cooldown_left: 0,
            },
        ),
        UnitKind::Heavy => (
            Health::full(Fixed::from_int(220)),
            Weapon {
                range: Fixed::from_int(12),
                damage: Fixed::from_int(22),
                cooldown_ticks: 60,
                cooldown_left: 0,
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
        assert_eq!(res.get(Faction::Player), 40, "rejected spend must not debit");
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
        // Floor is respected at a very high (saturated) level.
        assert_eq!(prod_time(UnitKind::Rifleman, 255), PROD_TICKS_FLOOR);
        assert_eq!(prod_time(UnitKind::Heavy, 255), PROD_TICKS_FLOOR);
    }
}

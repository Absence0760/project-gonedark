//! Ammo resupply (D67) — the logistics half of all-unit ammo (invariant #1, fixed-point).
//!
//! All-unit ammo ([`crate::combat`]) gives every magazine weapon a finite carried `reserve`; once it
//! is spent the unit is combat-ineffective. This system is how it rearms: each tick, every alive
//! combatant standing within [`RESUPPLY_RANGE`] of a *friendly, finished* building (a Camp or
//! Barracks — the supply points) tops its [`Weapon::reserve`](crate::components::Weapon::reserve)
//! back up by [`RESUPPLY_PER_TICK`], capped at its `reserve_max`. Pull a depleted squad back to base;
//! it rearms, then the combat auto-reload feeds the magazine from the refilled reserve.
//!
//! It runs as its own system in [`Sim::step`](crate::sim::Sim::step) — after combat/heal have
//! settled this tick, before economy — reading and writing only sim state in stable index order with
//! no float and no RNG, so its effect (the `reserve` purse, already folded by the checksum) is
//! peer-identical (invariant #7). A world with no building, or with every unit already at full
//! reserve / carrying no magazine, is a no-op — so a building-free or ammo-free scene's checksum
//! stream is byte-unchanged.
//!
//! Determinism note: resupply is *monotone to the cap* — a unit in range of two supply points still
//! gains exactly [`RESUPPLY_PER_TICK`] (range is a boolean "near supply?", not a per-source stack),
//! and the `min(.., reserve_max)` clamp lands identically whatever the scan order, so the fixed
//! index-order walk is the canonical (and only) order on every peer.

use crate::components::{BuildingKind, EntityKind};
use crate::ecs::World;
use crate::fixed::Fixed;

/// Distance (world units) within which a friendly supply building rearms a unit. Generous (8) so a
/// squad pulled back near its base rearms without pixel-perfect parking — slightly past the Medic's
/// [`HEAL_RADIUS`](crate::heal::HEAL_RADIUS) (6), since a base resupplies a wider apron than a medic.
pub const RESUPPLY_RANGE: Fixed = Fixed::from_int(8);

/// Reserve rounds restored per tick while in range of a supply building. `2`/tick = 120 rounds/s at
/// the locked 60 Hz — a full Rifleman reserve (180) refills in ~1.5 s, fast enough that rearming is a
/// brief lull, slow enough that staying topped-up means staying near base (the logistics tension). A
/// playtest baseline (not `--metrics`-measured).
pub const RESUPPLY_PER_TICK: u16 = 2;

/// Is `kind` a supply point — a building units can rearm at? Both production buildings double as
/// supply: the Camp (main base) and the forward Barracks.
#[inline]
fn is_supply_point(kind: BuildingKind) -> bool {
    matches!(kind, BuildingKind::Camp | BuildingKind::Barracks)
}

/// Advance one tick of resupply. For each alive combatant unit below its reserve cap, if any
/// friendly *finished* supply building is within [`RESUPPLY_RANGE`], add [`RESUPPLY_PER_TICK`] to its
/// reserve (clamped at `reserve_max`). Stable index-order scan, integer/`Fixed` only — deterministic
/// (invariant #1/#7). A `mag_size == 0` weapon (the Medic / infinite-ammo test units) carries no
/// magazine, so it is skipped.
pub fn resupply_system(world: &mut World) {
    let range_sq = RESUPPLY_RANGE * RESUPPLY_RANGE;
    let cap = world.capacity();

    // Collect the finished supply buildings once (few of them) so the per-unit check is O(buildings),
    // not O(capacity). Faction is checked per-unit below, so this holds buildings of every faction.
    let supply: Vec<usize> = (0..cap)
        .filter(|&b| {
            world.is_index_alive(b)
                && world.kind[b] == EntityKind::Building
                && is_supply_point(world.building[b].kind)
                && world.building[b].build_ticks_left == 0
        })
        .collect();
    if supply.is_empty() {
        return;
    }

    for i in 0..cap {
        if !world.is_index_alive(i) || world.kind[i] != EntityKind::Unit {
            continue;
        }
        let w = world.weapon[i];
        // No magazine (Medic / infinite-ammo units), or already at full reserve: nothing to do.
        if w.mag_size == 0 || w.reserve >= w.reserve_max {
            continue;
        }
        let unit_pos = world.pos[i];
        let unit_faction = world.faction[i];
        let near_supply = supply.iter().any(|&b| {
            world.faction[b] == unit_faction
                && (world.pos[b] - unit_pos).len_sq() <= range_sq
        });
        if near_supply {
            let add = RESUPPLY_PER_TICK.min(w.reserve_max - w.reserve);
            world.weapon[i].reserve += add;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Building, BuildingKind, Faction, Health, Vec2, Weapon};
    use crate::fixed::Fixed;

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    /// Spawn a combatant unit at `(x, y)` with a magazine weapon whose reserve is `reserve` of
    /// `reserve_max`.
    fn spawn_unit(world: &mut World, x: i32, y: i32, faction: Faction, reserve: u16, reserve_max: u16) -> usize {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.health[i] = Health::full(fx(100));
        world.weapon[i] = Weapon {
            range: fx(10),
            damage: fx(5),
            mag_size: 30,
            ammo: 30,
            reserve,
            reserve_max,
            ..Default::default()
        };
        i
    }

    /// Spawn a `kind` building at `(x, y)`, finished unless `build_ticks_left` says otherwise.
    fn spawn_building(world: &mut World, x: i32, y: i32, faction: Faction, kind: BuildingKind, build_ticks_left: u16) -> usize {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.health[i] = Health::full(fx(1000));
        world.kind[i] = EntityKind::Building;
        world.building[i] = Building {
            kind,
            level: 0,
            build_ticks_left,
            queue: Vec::new(),
        };
        i
    }

    #[test]
    fn unit_near_friendly_camp_gains_reserve_per_tick_capped() {
        let mut world = World::new();
        let unit = spawn_unit(&mut world, 0, 0, Faction::Player, 10, 180);
        spawn_building(&mut world, 3, 0, Faction::Player, BuildingKind::Camp, 0);

        resupply_system(&mut world);
        assert_eq!(world.weapon[unit].reserve, 10 + RESUPPLY_PER_TICK, "rearms each tick");
        for _ in 0..1000 {
            resupply_system(&mut world);
        }
        assert_eq!(world.weapon[unit].reserve, 180, "reserve never exceeds reserve_max");
    }

    #[test]
    fn barracks_is_also_a_supply_point() {
        let mut world = World::new();
        let unit = spawn_unit(&mut world, 0, 0, Faction::Player, 0, 180);
        spawn_building(&mut world, 5, 0, Faction::Player, BuildingKind::Barracks, 0);

        resupply_system(&mut world);
        assert_eq!(world.weapon[unit].reserve, RESUPPLY_PER_TICK, "a forward Barracks rearms too");
    }

    #[test]
    fn unit_out_of_range_of_any_building_gains_nothing() {
        let mut world = World::new();
        // Building at distance 9 > RESUPPLY_RANGE (8).
        let unit = spawn_unit(&mut world, 0, 0, Faction::Player, 10, 180);
        spawn_building(&mut world, 9, 0, Faction::Player, BuildingKind::Camp, 0);

        resupply_system(&mut world);
        assert_eq!(world.weapon[unit].reserve, 10, "no supply in range → no rearm");
    }

    #[test]
    fn enemy_building_never_resupplies() {
        let mut world = World::new();
        let unit = spawn_unit(&mut world, 0, 0, Faction::Player, 10, 180);
        spawn_building(&mut world, 2, 0, Faction::Enemy, BuildingKind::Camp, 0);

        resupply_system(&mut world);
        assert_eq!(world.weapon[unit].reserve, 10, "you do not rearm at the enemy's base");
    }

    #[test]
    fn unfinished_building_does_not_resupply() {
        let mut world = World::new();
        let unit = spawn_unit(&mut world, 0, 0, Faction::Player, 10, 180);
        // Still under construction (build_ticks_left > 0).
        spawn_building(&mut world, 2, 0, Faction::Player, BuildingKind::Camp, 100);

        resupply_system(&mut world);
        assert_eq!(world.weapon[unit].reserve, 10, "a half-built base has no stores yet");
    }

    #[test]
    fn full_reserve_and_magless_units_are_left_alone() {
        let mut world = World::new();
        let full = spawn_unit(&mut world, 0, 0, Faction::Player, 180, 180);
        // A magazine-less unit (mag_size 0 = infinite ammo / Medic): never resupplied.
        let magless = spawn_unit(&mut world, 1, 0, Faction::Player, 0, 0);
        world.weapon[magless].mag_size = 0;
        spawn_building(&mut world, 2, 0, Faction::Player, BuildingKind::Camp, 0);

        resupply_system(&mut world);
        assert_eq!(world.weapon[full].reserve, 180, "a full reserve is not over-filled");
        assert_eq!(world.weapon[magless].reserve, 0, "a mag-less weapon has nothing to resupply");
    }

    #[test]
    fn building_free_world_is_a_noop() {
        let mut world = World::new();
        let unit = spawn_unit(&mut world, 0, 0, Faction::Player, 10, 180);
        resupply_system(&mut world);
        assert_eq!(world.weapon[unit].reserve, 10, "no supply points anywhere → no change");
    }
}

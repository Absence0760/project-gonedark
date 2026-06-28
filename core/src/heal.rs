//! Medic healing (D65) — the Medic support mechanic. Deterministic, fixed-point (invariant #1).
//!
//! Each tick, every alive [`Medic`](crate::components::UnitKind::Medic) heals friendly **units**
//! within [`HEAL_RADIUS`] by [`HEAL_PER_TICK`], capped at their max HP. It runs as its own system in
//! [`Sim::step`](crate::sim::Sim::step) — *after* combat/projectiles have settled this tick's damage
//! and despawned the dead (so a Medic never heals a corpse), and before territory/economy. It reads
//! and writes only sim state, in stable index order, with no float and no RNG, so its effect (the
//! `health` purse) folds into the per-tick checksum exactly like combat (invariant #7).
//!
//! A world with **no Medic** is a no-op — the outer scan finds nothing — so every pre-existing
//! scene's checksum stream is byte-unchanged (the Medic is opt-in by its mere absence).
//!
//! Determinism note: healing is *commutative at the cap* — two Medics healing the same target add
//! the same total whatever the order, and the `min(.., max)` clamp lands on `max` regardless — so
//! the fixed index-order scan is the canonical (and only) order, identical on every peer.

use crate::components::{EntityKind, UnitKind, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;

/// Radius (world units) within which a Medic heals a friendly unit. Matches the capture radius (6) —
/// a Medic mends the squad it stands with.
pub const HEAL_RADIUS: Fixed = Fixed::from_int(6);

/// HP restored per tick to each friendly in range. `1/8` HP/tick = 7.5 HP/s at the locked 60 Hz —
/// meaningful sustain (it mends a 100-HP Rifleman in ~13 s) without out-healing sustained rifle fire
/// (a Rifleman deals 12 DPS), so a Medic tips attrition without making a squad unkillable. A playtest
/// baseline (not `--metrics`-measured).
pub const HEAL_PER_TICK: Fixed = Fixed::from_ratio(1, 8);

/// Advance one tick of Medic healing. For each alive Medic, every friendly **unit** (same faction,
/// not the Medic itself, not a building) within [`HEAL_RADIUS`] that is alive is healed by
/// [`HEAL_PER_TICK`] (capped at its max via [`Health::heal`](crate::components::Health::heal)).
/// Stable index-order scan, fixed-point only — deterministic (invariant #1/#7).
pub fn heal_system(world: &mut World) {
    let radius_sq = HEAL_RADIUS * HEAL_RADIUS;
    let cap = world.capacity();
    for mi in 0..cap {
        if !world.is_index_alive(mi)
            || world.kind[mi] != EntityKind::Unit
            || world.unit_kind[mi] != UnitKind::Medic
        {
            continue;
        }
        let medic_pos: Vec2 = world.pos[mi];
        let medic_faction = world.faction[mi];
        for ti in 0..cap {
            if ti == mi || !world.is_index_alive(ti) {
                continue;
            }
            // Friendly units only — never buildings, never another faction.
            if world.kind[ti] != EntityKind::Unit || world.faction[ti] != medic_faction {
                continue;
            }
            // Read the target position (copied out) before the mutable heal write below — disjoint
            // sequential field accesses, no aliasing.
            if (world.pos[ti] - medic_pos).len_sq() > radius_sq {
                continue;
            }
            world.health[ti].heal(HEAL_PER_TICK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Faction, Health};
    use crate::ecs::{Entity, World};

    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    /// Spawn a unit of `kind`/`faction` at `pos` with explicit current/max HP; return its handle.
    fn spawn(world: &mut World, kind: UnitKind, faction: Faction, pos: Vec2, cur: i32, max: i32) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.kind[i] = EntityKind::Unit;
        world.unit_kind[i] = kind;
        world.faction[i] = faction;
        world.pos[i] = pos;
        world.health[i] = Health {
            cur: Fixed::from_int(cur),
            max: Fixed::from_int(max),
        };
        e
    }

    fn hp(world: &World, e: Entity) -> Fixed {
        world.health[e.index as usize].cur
    }

    #[test]
    fn medic_heals_a_damaged_friendly_over_ticks_capped_at_max() {
        let mut world = World::new();
        let _medic = spawn(&mut world, UnitKind::Medic, Faction::Player, at(0, 0), 90, 90);
        let hurt = spawn(&mut world, UnitKind::Rifleman, Faction::Player, at(2, 0), 50, 100);

        // One tick heals by exactly HEAL_PER_TICK.
        heal_system(&mut world);
        assert_eq!(hp(&world, hurt), Fixed::from_int(50) + HEAL_PER_TICK);

        // Driven long enough, it tops out at max and never overshoots.
        for _ in 0..10_000 {
            heal_system(&mut world);
        }
        assert_eq!(hp(&world, hurt), Fixed::from_int(100), "heal caps at max HP");
    }

    #[test]
    fn medic_does_not_heal_enemies() {
        let mut world = World::new();
        let _medic = spawn(&mut world, UnitKind::Medic, Faction::Player, at(0, 0), 90, 90);
        let enemy = spawn(&mut world, UnitKind::Rifleman, Faction::Enemy, at(1, 0), 50, 100);
        for _ in 0..50 {
            heal_system(&mut world);
        }
        assert_eq!(hp(&world, enemy), Fixed::from_int(50), "an enemy is never healed");
    }

    #[test]
    fn medic_does_not_heal_out_of_radius() {
        let mut world = World::new();
        let _medic = spawn(&mut world, UnitKind::Medic, Faction::Player, at(0, 0), 90, 90);
        // HEAL_RADIUS is 6 → a friendly at (7,0) (len_sq 49 > 36) is out of range.
        let far = spawn(&mut world, UnitKind::Rifleman, Faction::Player, at(7, 0), 50, 100);
        for _ in 0..50 {
            heal_system(&mut world);
        }
        assert_eq!(hp(&world, far), Fixed::from_int(50), "out-of-radius friendly is not healed");
    }

    #[test]
    fn medic_does_not_heal_a_full_friendly_or_revive_the_dead() {
        let mut world = World::new();
        let _medic = spawn(&mut world, UnitKind::Medic, Faction::Player, at(0, 0), 90, 90);
        let full = spawn(&mut world, UnitKind::Rifleman, Faction::Player, at(1, 0), 100, 100);
        // A dead unit (cur 0) must NOT be revived — heal is a no-op on a corpse.
        let dead = spawn(&mut world, UnitKind::Rifleman, Faction::Player, at(1, 1), 0, 100);
        heal_system(&mut world);
        assert_eq!(hp(&world, full), Fixed::from_int(100), "a full unit stays full (no overshoot)");
        assert_eq!(hp(&world, dead), Fixed::ZERO, "heal never revives a dead unit");
    }

    #[test]
    fn no_medic_is_a_no_op() {
        // The opt-in-by-absence property: with no Medic present, healing changes nothing — so every
        // Medic-free scene's checksum is byte-unchanged.
        let mut world = World::new();
        let hurt = spawn(&mut world, UnitKind::Rifleman, Faction::Player, at(0, 0), 50, 100);
        spawn(&mut world, UnitKind::Heavy, Faction::Player, at(1, 0), 100, 300);
        heal_system(&mut world);
        assert_eq!(hp(&world, hurt), Fixed::from_int(50), "no medic ⇒ no healing");
    }

    #[test]
    fn two_medics_on_one_target_heal_commutatively() {
        // Determinism rests on healing being order-independent: two Medics on one target add the same
        // total regardless of scan order (and the cap lands on max either way).
        let mut world = World::new();
        spawn(&mut world, UnitKind::Medic, Faction::Player, at(-1, 0), 90, 90);
        spawn(&mut world, UnitKind::Medic, Faction::Player, at(1, 0), 90, 90);
        let hurt = spawn(&mut world, UnitKind::Rifleman, Faction::Player, at(0, 0), 50, 100);
        heal_system(&mut world);
        assert_eq!(
            hp(&world, hurt),
            Fixed::from_int(50) + HEAL_PER_TICK + HEAL_PER_TICK,
            "two medics heal twice the per-tick amount this tick"
        );
    }
}

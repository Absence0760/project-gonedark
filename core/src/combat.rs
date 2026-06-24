//! Combat, suppression, cover, and death (invariants #1, #3 — fixed-point, literal AI).
//!
//! `combat_system` is the deterministic per-tick weapons resolver. For each living, armed
//! unit it acquires a target (nearest enemy in weapon range with line of sight), respecting
//! the unit's [`Stance`](crate::components::Stance), fires on cooldown, applies
//! cover-mitigated damage, accumulates **suppression** on the target, decays suppression
//! over time, and despawns anything reduced to zero health — emitting [`SimEvent`]s for the
//! alert/audio channel as it goes.
//!
//! Determinism rules it must hold (the determinism guard greps this file):
//! - Fixed-point only; no floats, no `std`/`libm` transcendentals (use `trig`/`Fixed`).
//! - Iterate entities in **stable index order**; break target ties to the lowest index.
//! - All randomness via the passed `&mut Rng` (integer draws only), never wall-clock.
//!
//! The literal-executor rule (invariant #3) still binds: combat acts on the *stance* the
//! player set, it does not invent targets the stance forbids or chase beyond weapon range.
//!
//! IMPLEMENTATION OWNER: worker 2. A real generational [`Entity`] handle for the
//! shooter/target (needed by `last_attacker` and the `SimEvent`s) comes from the O(1)
//! [`World::entity`] accessor.

use crate::components::{EntityKind, Faction, InputSource, Stance};
use crate::ecs::World;
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::rng::Rng;
use crate::terrain::Terrain;

/// Suppression at or above this fraction of [`SUPPRESSION_MAX`] pins a unit: it may not fire
/// and (per `orders`) moves at reduced speed. (Tunable; worker 2 may refine.)
pub const SUPPRESSION_PIN: Fixed = Fixed::from_ratio(3, 4);

/// Ceiling for accumulated suppression; it decays toward zero each tick.
pub const SUPPRESSION_MAX: Fixed = Fixed::ONE;

/// Suppression removed per tick when not taking fire.
pub const SUPPRESSION_DECAY: Fixed = Fixed::from_ratio(1, 64);

/// Suppression added to a target per shot that lands.
pub const SUPPRESSION_PER_HIT: Fixed = Fixed::from_ratio(1, 8);


/// Is `(attacker, defender)` a hostile pair? Combat engages only across distinct factions and
/// never involves `Neutral` on either side (invariant #3 keeps it literal — no friendly fire,
/// no neutral aggression).
#[inline]
fn is_enemy(attacker: Faction, defender: Faction) -> bool {
    attacker != defender && attacker != Faction::Neutral && defender != Faction::Neutral
}

/// Can `shooter_idx` currently land a shot on `target_idx`? Target must be alive, a different
/// hostile faction, within `range` (squared compare — never a sqrt), and in line of sight.
fn can_engage(world: &World, terrain: &Terrain, shooter_idx: usize, target_idx: usize) -> bool {
    if shooter_idx == target_idx || !world.is_index_alive(target_idx) {
        return false;
    }
    if world.health[target_idx].is_dead() {
        return false;
    }
    if !is_enemy(world.faction[shooter_idx], world.faction[target_idx]) {
        return false;
    }
    let my_pos = world.pos[shooter_idx];
    let target_pos = world.pos[target_idx];
    let range = world.weapon[shooter_idx].range;
    let dist_sq = (target_pos - my_pos).len_sq();
    if dist_sq > range * range {
        return false;
    }
    terrain.line_of_sight(my_pos, target_pos)
}

/// Pick the target slot for `shooter_idx` under its stance, or `None` to hold fire.
/// `FireAtWill` takes the nearest valid enemy, ties broken to the lowest index (we only
/// replace the best on a strictly-smaller squared distance). `ReturnFire` engages only its
/// recorded `last_attacker` (and only if that attacker is still a valid target). `HoldFire`
/// never fires.
fn acquire_target(world: &World, terrain: &Terrain, shooter_idx: usize) -> Option<usize> {
    match world.stance[shooter_idx] {
        Stance::HoldFire => None,
        Stance::ReturnFire => {
            let attacker = world.last_attacker[shooter_idx]?;
            let target_idx = attacker.index as usize;
            // The stored handle must still be the live occupant of that slot, and a valid
            // target right now (in range / LoS / hostile). Otherwise: do nothing.
            if world.is_alive(attacker) && can_engage(world, terrain, shooter_idx, target_idx) {
                Some(target_idx)
            } else {
                None
            }
        }
        Stance::FireAtWill => {
            let my_pos = world.pos[shooter_idx];
            let mut best: Option<(usize, Fixed)> = None;
            for target_idx in 0..world.capacity() {
                if !can_engage(world, terrain, shooter_idx, target_idx) {
                    continue;
                }
                let dist_sq = (world.pos[target_idx] - my_pos).len_sq();
                match best {
                    // Strictly-less keeps the lowest index on a tie (we scan ascending).
                    Some((_, best_sq)) if dist_sq >= best_sq => {}
                    _ => best = Some((target_idx, dist_sq)),
                }
            }
            best.map(|(idx, _)| idx)
        }
    }
}

/// Resolve one tick of combat over `world`, using `terrain` for cover + line of sight and
/// `rng` for any deterministic rolls, pushing facts into `events`.
///
/// Three index-ordered passes for clean, peer-identical ordering:
/// 1. **Upkeep** — decay suppression toward zero; tick weapon cooldowns down.
/// 2. **Engage** — armed, non-embodied, non-pinned units acquire a target by stance and fire
///    on a ready cooldown, applying cover-mitigated damage + suppression.
/// 3. **Deaths** — anything at zero health emits `Killed` and is despawned.
pub fn combat_system(
    world: &mut World,
    terrain: &Terrain,
    rng: &mut Rng,
    events: &mut Vec<SimEvent>,
) {
    let _ = rng; // No stochastic combat yet; reserved for future spread/crit rolls.
    let n = world.capacity();

    // --- Pass 1: upkeep (every alive entity) ---
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        let s = world.suppression[i];
        world.suppression[i] = (s - SUPPRESSION_DECAY).max(Fixed::ZERO);
        if world.weapon[i].cooldown_left > 0 {
            world.weapon[i].cooldown_left -= 1;
        }
    }

    // --- Pass 2: engage (armed, order-driven, un-pinned units) ---
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        if world.kind[i] != EntityKind::Unit {
            continue;
        }
        if world.input_source[i] == InputSource::Embodied {
            continue;
        }
        if world.weapon[i].range <= Fixed::ZERO {
            continue;
        }
        // Pinned by suppression: may not fire (orders also slows it elsewhere).
        if world.suppression[i] >= SUPPRESSION_PIN {
            continue;
        }

        let target_idx = match acquire_target(world, terrain, i) {
            Some(t) => t,
            None => continue,
        };

        // Cooldown gates the rate of fire; a target may be held but not shot this tick.
        if world.weapon[i].cooldown_left != 0 {
            continue;
        }

        let shooter = match world.entity(i) {
            Some(e) => e,
            None => continue,
        };
        let target = match world.entity(target_idx) {
            Some(e) => e,
            None => continue,
        };

        let mult = terrain.cover_at(world.pos[target_idx]).damage_multiplier();
        let damage = world.weapon[i].damage * mult;

        world.health[target_idx].cur -= damage;
        world.last_attacker[target_idx] = Some(shooter);
        world.suppression[target_idx] =
            (world.suppression[target_idx] + SUPPRESSION_PER_HIT).min(SUPPRESSION_MAX);
        world.weapon[i].cooldown_left = world.weapon[i].cooldown_ticks;

        events.push(SimEvent::Damaged {
            entity: target,
            faction: world.faction[target_idx],
            source: shooter,
            amount: damage,
            pos: world.pos[target_idx],
        });
    }

    // --- Pass 3: deaths (anything reduced to zero) ---
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        if !world.health[i].is_dead() {
            continue;
        }
        let dead = match world.entity(i) {
            Some(e) => e,
            None => continue,
        };
        // The killer is the last entity to hit it, when that handle is still live.
        let source = match world.last_attacker[i] {
            Some(a) if world.is_alive(a) => a,
            _ => dead, // self-attributed when the attacker is gone (e.g. mutual kill).
        };
        events.push(SimEvent::Killed {
            entity: dead,
            faction: world.faction[i],
            source,
            pos: world.pos[i],
        });
        world.despawn(dead);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Faction, Health, Stance, Vec2, Weapon};
    use crate::ecs::Entity;
    use crate::terrain::{Cover, Terrain};

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    /// Spawn a combat unit at `(x, y)` with the given faction/weapon, full at `hp`.
    fn spawn_unit(
        world: &mut World,
        x: i32,
        y: i32,
        faction: Faction,
        hp: i32,
        weapon: Weapon,
    ) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.health[i] = Health::full(fx(hp));
        world.weapon[i] = weapon;
        e
    }

    fn rifle(range: i32, damage: i32, cooldown: u16) -> Weapon {
        Weapon {
            range: fx(range),
            damage: fx(damage),
            cooldown_ticks: cooldown,
            cooldown_left: 0,
        }
    }

    fn run(world: &mut World, terrain: &Terrain, events: &mut Vec<SimEvent>) {
        let mut rng = Rng::new(1);
        combat_system(world, terrain, &mut rng, events);
    }

    #[test]
    fn cover_damage_multiplier_math() {
        assert_eq!(Cover::None.damage_multiplier(), Fixed::ONE);
        assert_eq!(Cover::Light.damage_multiplier(), Fixed::from_ratio(1, 2));
        assert_eq!(Cover::Heavy.damage_multiplier(), Fixed::from_ratio(1, 4));
    }

    #[test]
    fn fire_at_will_open_terrain_full_damage_kills_and_despawns() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 3, 0, Faction::Enemy, 100, Weapon::default());
        world.stance[enemy.index as usize] = Stance::HoldFire;

        // 25 dmg/tick at full multiplier (open) -> dead after 4 hits.
        let mut events = Vec::new();
        for _ in 0..3 {
            run(&mut world, &terrain, &mut events);
        }
        assert!(world.is_alive(enemy), "enemy should survive 3 hits of 75");
        assert_eq!(world.health[enemy.index as usize].cur, fx(25));
        // Every hit recorded a Damaged of full 25 from the shooter.
        let damaged: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SimEvent::Damaged { .. }))
            .collect();
        assert_eq!(damaged.len(), 3);
        if let SimEvent::Damaged {
            amount, source, ..
        } = damaged[0]
        {
            assert_eq!(*amount, fx(25), "open terrain = full damage");
            assert_eq!(*source, shooter);
        }

        // 4th hit kills.
        run(&mut world, &terrain, &mut events);
        assert!(!world.is_alive(enemy), "enemy should be dead + despawned");
        let kills: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SimEvent::Killed { .. }))
            .collect();
        assert_eq!(kills.len(), 1);
        if let SimEvent::Killed { entity, source, faction, .. } = kills[0] {
            assert_eq!(*entity, enemy);
            assert_eq!(*source, shooter);
            assert_eq!(*faction, Faction::Enemy);
        }
    }

    #[test]
    fn last_attacker_recorded_on_hit() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.last_attacker[enemy.index as usize], Some(shooter));
    }

    #[test]
    fn hold_fire_never_deals_damage() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::HoldFire;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[enemy.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn return_fire_idle_until_attacked_then_fires_back() {
        let mut world = World::new();
        let terrain = Terrain::open();
        // Defender is ReturnFire and armed; attacker is FireAtWill.
        let defender = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 0));
        world.stance[defender.index as usize] = Stance::ReturnFire;
        let attacker = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, rifle(10, 10, 0));
        world.stance[attacker.index as usize] = Stance::HoldFire; // doesn't shoot yet

        // With the attacker holding fire, the ReturnFire defender has no last_attacker -> idle.
        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[attacker.index as usize].cur, fx(100));
        assert!(world.last_attacker[defender.index as usize].is_none());

        // Now the attacker opens fire: it hits the defender, recording last_attacker.
        world.stance[attacker.index as usize] = Stance::FireAtWill;
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.last_attacker[defender.index as usize],
            Some(attacker)
        );

        // Next tick the defender returns fire against its recorded attacker.
        run(&mut world, &terrain, &mut events);
        assert!(
            world.health[attacker.index as usize].cur < fx(100),
            "ReturnFire defender should now be hitting back"
        );
        assert_eq!(world.last_attacker[attacker.index as usize], Some(defender));
    }

    #[test]
    fn out_of_range_enemy_never_hit() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(5, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Distance 6 > range 5.
        let enemy = spawn_unit(&mut world, 6, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[enemy.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn just_in_range_enemy_is_hit() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(5, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Distance exactly 5 == range 5 -> dist_sq == range*range -> engages.
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(75));
    }

    #[test]
    fn same_faction_never_fights() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let a = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[a.index as usize] = Stance::FireAtWill;
        let b = spawn_unit(&mut world, 2, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[b.index as usize] = Stance::FireAtWill;

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[a.index as usize].cur, fx(100));
        assert_eq!(world.health[b.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn neutral_never_fights() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let player = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[player.index as usize] = Stance::FireAtWill;
        let neutral = spawn_unit(&mut world, 2, 0, Faction::Neutral, 100, rifle(10, 25, 0));
        world.stance[neutral.index as usize] = Stance::FireAtWill;

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[player.index as usize].cur, fx(100));
        assert_eq!(world.health[neutral.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn cooldown_gates_fire_rate() {
        let mut world = World::new();
        let terrain = Terrain::open();
        // Cooldown of 3 ticks: fire on tick 0, then nothing for 3 ticks, then fire again.
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 3));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 1000, Weapon::default());

        let mut events = Vec::new();
        // Tick 1: fires, cooldown_left set to 3.
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(990));
        // Ticks 2-3: upkeep decrements 3->2 then 2->1; engage sees cd > 0 -> no fire.
        run(&mut world, &terrain, &mut events);
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(990),
            "no fire while on cooldown"
        );
        // Tick 4: upkeep decrements 1->0, engage now sees cd == 0 -> fires again.
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(980),
            "fires exactly cooldown_ticks after the previous shot"
        );
    }

    #[test]
    fn suppression_accumulates_on_target() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 1, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 1000, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        // One hit: +1/8 suppression, then upkeep next tick would decay; check right after first
        // tick. Pass-1 upkeep runs before pass-2 fire within the same tick, so the freshly
        // applied SUPPRESSION_PER_HIT has not decayed yet this tick.
        let s = world.suppression[enemy.index as usize];
        assert_eq!(s, SUPPRESSION_PER_HIT);

        // Several more hits push suppression up (net of the 1/64 per-tick decay).
        for _ in 0..6 {
            run(&mut world, &terrain, &mut events);
        }
        assert!(
            world.suppression[enemy.index as usize] > SUPPRESSION_PER_HIT,
            "suppression should accumulate over repeated hits"
        );
    }

    #[test]
    fn fully_suppressed_unit_does_not_fire() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Pin the shooter at max suppression; upkeep decays only 1/64, so it stays >= PIN.
        world.suppression[shooter.index as usize] = SUPPRESSION_MAX;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(100),
            "pinned unit must not fire"
        );
        assert!(world.suppression[shooter.index as usize] >= SUPPRESSION_PIN);
    }

    #[test]
    fn suppression_decays_toward_zero_and_clamps() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let lone = spawn_unit(&mut world, 0, 0, Faction::Player, 100, Weapon::default());
        // Below one decay step: must clamp to zero, never go negative.
        world.suppression[lone.index as usize] = Fixed::from_ratio(1, 128);

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.suppression[lone.index as usize], Fixed::ZERO);
    }

    #[test]
    fn embodied_unit_does_not_auto_fire() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(100),
            "embodied units fire by live input, not the resolver"
        );
    }

    #[test]
    fn fire_at_will_breaks_ties_to_lowest_index() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Two enemies equidistant (distance 3) — lower index must be chosen.
        let near_low = spawn_unit(&mut world, 3, 0, Faction::Enemy, 100, Weapon::default());
        let near_high = spawn_unit(&mut world, 0, 3, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[near_low.index as usize].cur, fx(75));
        assert_eq!(world.health[near_high.index as usize].cur, fx(100));
    }

    #[test]
    fn world_entity_recovers_handle_after_recycle() {
        // combat builds `last_attacker`/event handles via `World::entity`; confirm it returns
        // the live generation for a recycled slot and `None` once the slot is dead.
        let mut world = World::new();
        let a = world.spawn();
        world.despawn(a);
        let b = world.spawn();
        world.despawn(b);
        let c = world.spawn();
        assert_eq!(c.generation, 2);
        assert_eq!(world.entity(c.index as usize), Some(c));
        world.despawn(c);
        assert_eq!(world.entity(0), None);
    }
}

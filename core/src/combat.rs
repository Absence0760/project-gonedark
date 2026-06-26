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

use crate::components::{EntityKind, Faction, InputSource, Stance, Vec2};
use crate::ecs::World;
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::rng::Rng;
use crate::spatial::SpatialHash;
use crate::terrain::Terrain;

/// Suppression at or above this fraction of [`SUPPRESSION_MAX`] pins a unit: it may not fire
/// and (per `orders`) moves at reduced speed.
///
/// 1/2 (D30, lowered from 3/4). At [`SUPPRESSION_PER_HIT`] = 1/8 this means a unit pins once
/// **four** shots land before they decay — i.e. *concentrated* fire pins, but a lone shooter
/// (one hit per cooldown, decaying 1/64 a tick) never accumulates enough, so a clean 1v1 still
/// resolves by damage. The harness confirmed the old 3/4 pin never triggered before a kill in
/// focus-fire (suppression was cosmetic); at 1/2 a 4-shooter focus pins the target on the first
/// burst, *before* it dies — making suppression a real "concentrate fire to pin" lever (D26 goal).
pub const SUPPRESSION_PIN: Fixed = Fixed::from_ratio(1, 2);

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
///
/// There is deliberately **no `EntityKind` filter on the target**: a unit may engage an enemy
/// *building* (so an attack-moving squad razes a hostile camp), and `is_enemy` still spares
/// friendly buildings. Only the *attacker* is restricted to `EntityKind::Unit` (in the engage
/// pass) — buildings never shoot.
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
/// `FireAtWill` takes the nearest valid enemy, ties broken to the lowest index. `ReturnFire`
/// engages only its recorded `last_attacker` (and only if that attacker is still a valid
/// target). `HoldFire` never fires.
///
/// `FireAtWill` queries the per-tick [`SpatialHash`] instead of scanning all units (O(n²) →
/// near-O(n)), but the result is **bit-identical** to the old brute-force scan: the hash's
/// `(dist_sq, idx)` lexicographic comparator reproduces the same min-distance/lowest-index pick,
/// and `can_engage` remains the sole authoritative range/LoS/hostility filter.
fn acquire_target(
    world: &World,
    terrain: &Terrain,
    spatial: &SpatialHash,
    shooter_idx: usize,
) -> Option<usize> {
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
            let range = world.weapon[shooter_idx].range;
            spatial.nearest_within(
                my_pos,
                range,
                |idx| can_engage(world, terrain, shooter_idx, idx),
                |idx| (world.pos[idx] - my_pos).len_sq(),
            )
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

    // --- Build the per-tick spatial index for target acquisition (A5) ---
    // Positions are fixed for the whole engage pass (pass 2 mutates only health/suppression/
    // cooldown/last_attacker, never `pos`), so one build serves every shooter this tick. It is
    // NOT sim state — never folded into the checksum, exactly like `flow_field::FlowFieldCache`.
    let spatial = SpatialHash::build(world);

    // --- Pass 2: engage (armed, order-driven, un-pinned units) ---
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        // Only units attack — buildings never acquire targets or fire (they are damageable
        // TARGETS only; see `can_engage`). This is the sole attacker-side kind filter.
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

        let target_idx = match acquire_target(world, terrain, &spatial, i) {
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

/// Cosine of the embodied weapon's half-cone angle, as a Fixed in `[0, 1]`. `cos(30°) ≈
/// 0.8660` — a 60°-wide aim cone, generous enough that a hip-fired shot reads as "I pointed at
/// him and hit," tight enough that you must actually face the target. Stored as the exact
/// rational `866/1000` so it is float-free and bit-identical on every peer (invariant #1). The
/// cone test squares this (never a sqrt), so only `cos²` ever enters the comparison.
pub const FIRE_CONE_COS_HALF: Fixed = Fixed::from_ratio(866, 1000);

/// Resolve one embodied shot from `shooter_idx` aimed along `dir` (a unit aim vector in Fixed
/// world space — quantized at the host boundary, invariant #1). A fixed-point **cone hitscan**:
/// the lowest-index living hostile entity — a unit **or** a building — that lies inside the aim
/// cone, within weapon range, and in line of sight takes the same cover-mitigated damage +
/// suppression the auto-resolver applies, and the weapon goes on cooldown. Returns silently (no
/// shot, no cooldown) if the weapon is disarmed, still cooling down, or no target qualifies.
/// Buildings are damageable so an embodied player can shoot down an enemy structure; the
/// `is_enemy` filter still spares friendly buildings (no own-base fire).
///
/// Determinism (the guard greps this file): fixed-point only, no sqrt/normalize. The cone test
/// `dir·(t−p) ≥ cos_half·|t−p|` is evaluated by **squaring both non-negative sides** —
/// `proj·proj ≥ cos_half²·|t−p|²` — after rejecting any target behind the aim (`proj < 0`), so a
/// transcendental never enters. Targets are scanned in stable index order; the first qualifier
/// (lowest index) wins ties. Only already-checksummed fields are written, so the per-tick
/// `fold()`/checksum stream is untouched (invariant #7).
pub fn resolve_fire(
    world: &mut World,
    terrain: &Terrain,
    shooter_idx: usize,
    dir: Vec2,
    events: &mut Vec<SimEvent>,
) {
    if !world.is_index_alive(shooter_idx) {
        return;
    }
    // A disarmed (range 0) weapon never fires; a hot weapon must finish its cooldown first. Both
    // mirror the auto-resolver's gates so embodied fire obeys the same rate-of-fire contract.
    if world.weapon[shooter_idx].range <= Fixed::ZERO {
        return;
    }
    if world.weapon[shooter_idx].cooldown_left != 0 {
        return;
    }

    let my_pos = world.pos[shooter_idx];
    let range = world.weapon[shooter_idx].range;
    let range_sq = range * range;
    let cos_half = FIRE_CONE_COS_HALF;
    let cos_half_sq = cos_half * cos_half;

    // Pick the lowest-index hostile, living target inside the cone, in range, with LoS. A target
    // may be a unit OR a building — embodied fire razes enemy structures (the `is_enemy` test
    // below still excludes friendly buildings). No kind filter here, on purpose.
    let mut chosen: Option<usize> = None;
    for t in 0..world.capacity() {
        if t == shooter_idx || !world.is_index_alive(t) {
            continue;
        }
        if world.health[t].is_dead() {
            continue;
        }
        if !is_enemy(world.faction[shooter_idx], world.faction[t]) {
            continue;
        }
        let to_target = world.pos[t] - my_pos;
        let dist_sq = to_target.len_sq();
        // Exclude a target sitting exactly on the muzzle (zero range): the cone test divides the
        // half-space by direction, and a zero vector has no direction. Treat it as out of arc.
        if dist_sq <= Fixed::ZERO || dist_sq > range_sq {
            continue;
        }
        // Cone test without a sqrt. `proj = dir·to_target`; reject anything behind the aim, then
        // compare squared: `proj² ≥ cos_half² · |to_target|²`.
        let proj = dir.dot(to_target);
        if proj < Fixed::ZERO {
            continue;
        }
        if proj * proj < cos_half_sq * dist_sq {
            continue;
        }
        if !terrain.line_of_sight(my_pos, world.pos[t]) {
            continue;
        }
        chosen = Some(t);
        break;
    }

    let target_idx = match chosen {
        Some(t) => t,
        None => return,
    };

    // Same writes the auto-resolver's engage pass performs: cover-mitigated damage, last-attacker,
    // suppression, and the weapon cooldown. Reusing them keeps embodied and AI fire identical
    // (and keeps every touched field already in the checksum fold).
    let shooter = match world.entity(shooter_idx) {
        Some(e) => e,
        None => return,
    };
    let target = match world.entity(target_idx) {
        Some(e) => e,
        None => return,
    };

    let mult = terrain.cover_at(world.pos[target_idx]).damage_multiplier();
    let damage = world.weapon[shooter_idx].damage * mult;

    world.health[target_idx].cur -= damage;
    world.last_attacker[target_idx] = Some(shooter);
    world.suppression[target_idx] =
        (world.suppression[target_idx] + SUPPRESSION_PER_HIT).min(SUPPRESSION_MAX);
    world.weapon[shooter_idx].cooldown_left = world.weapon[shooter_idx].cooldown_ticks;

    events.push(SimEvent::Damaged {
        entity: target,
        faction: world.faction[target_idx],
        source: shooter,
        amount: damage,
        pos: world.pos[target_idx],
    });
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

    /// Spawn a building at `(x, y)` with the given faction, full at `hp`. A building carries a
    /// default (range-0) weapon, so it is a valid damage TARGET but never an attacker.
    fn spawn_building(world: &mut World, x: i32, y: i32, faction: Faction, hp: i32) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.health[i] = Health::full(fx(hp));
        world.kind[i] = EntityKind::Building;
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
        if let SimEvent::Damaged { amount, source, .. } = damaged[0] {
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
        if let SimEvent::Killed {
            entity,
            source,
            faction,
            ..
        } = kills[0]
        {
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
        assert_eq!(world.last_attacker[defender.index as usize], Some(attacker));

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
    fn fire_at_will_ties_lowest_index_across_spatial_buckets() {
        // Three enemies equidistant (distance 5) but in DIFFERENT spatial-hash cells, so the
        // pick must not depend on which bucket the query reaches first — the lowest slot index
        // wins (the spatial query reproduces the brute-force scan's tie-break, A5).
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(20, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let near_low = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());
        let near_mid = spawn_unit(&mut world, 0, 5, Faction::Enemy, 100, Weapon::default());
        let near_high = spawn_unit(&mut world, -5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[near_low.index as usize].cur, fx(75), "lowest index hit");
        assert_eq!(world.health[near_mid.index as usize].cur, fx(100));
        assert_eq!(world.health[near_high.index as usize].cur, fx(100));
    }

    // --- Embodied fire: Command::Fire cone hitscan (resolve_fire) -----------------------------

    /// Aim straight along +X as a Fixed unit vector — no float ever constructed (invariant #1).
    fn aim_pos_x() -> Vec2 {
        Vec2::new(Fixed::ONE, Fixed::ZERO)
    }

    fn fire(world: &mut World, terrain: &Terrain, shooter: Entity, dir: Vec2, events: &mut Vec<SimEvent>) {
        resolve_fire(world, terrain, shooter.index as usize, dir, events);
    }

    #[test]
    fn fire_hits_target_inside_cone_in_range() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        // Directly ahead on +X, distance 5 (< range 10) — squarely inside the aim cone.
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(75), "open terrain = full damage");
        assert_eq!(world.last_attacker[enemy.index as usize], Some(shooter));
        assert_eq!(events.len(), 1, "one Damaged event for the hit");
    }

    #[test]
    fn fire_misses_target_outside_the_cone() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Aim +X, but the enemy is at +Y (90° off-axis) — well outside the ~30° half-cone.
        let enemy = spawn_unit(&mut world, 0, 5, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "off-axis target not hit");
        assert!(events.is_empty());
        // A clean miss must NOT spend the weapon's cooldown — you can re-aim and fire again.
        assert_eq!(world.weapon[shooter.index as usize].cooldown_left, 0);
    }

    #[test]
    fn fire_misses_target_behind_the_shooter() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Enemy directly BEHIND (−X) the +X aim: proj < 0, rejected before any squaring.
        let enemy = spawn_unit(&mut world, -5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn fire_misses_out_of_range_target() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(5, 25, 0));
        // On-axis and centered in the cone, but distance 6 > range 5.
        let enemy = spawn_unit(&mut world, 6, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "out of range = no hit");
        assert!(events.is_empty());
    }

    #[test]
    fn fire_blocked_by_line_of_sight() {
        let mut world = World::new();
        let mut terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        let enemy = spawn_unit(&mut world, 6, 0, Faction::Enemy, 100, Weapon::default());
        // Wall the cell strictly between the two (world (3,0)) with Heavy cover → blocks sight.
        let (wx, wy) = terrain.cell_of(Vec2::new(fx(3), fx(0)));
        terrain.set_cover(wx, wy, Cover::Heavy);

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "LoS-blocked shot misses");
        assert!(events.is_empty());
    }

    #[test]
    fn fire_respects_weapon_cooldown() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 5));
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 1000, Weapon::default());

        let mut events = Vec::new();
        // First shot lands and sets cooldown_left = 5.
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(990));
        assert_eq!(world.weapon[shooter.index as usize].cooldown_left, 5);
        // A second pull while hot does nothing (no damage, no event, cooldown unchanged).
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(990), "no fire while on cooldown");
        assert_eq!(events.len(), 1);
        assert_eq!(world.weapon[shooter.index as usize].cooldown_left, 5);
    }

    #[test]
    fn fire_skips_dead_and_finds_next_living_target() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Lower-index enemy already at zero health (dead but not yet despawned this tick) must be
        // skipped; the live higher-index enemy on-axis is hit instead.
        let dead = spawn_unit(&mut world, 4, 0, Faction::Enemy, 100, Weapon::default());
        world.health[dead.index as usize].cur = Fixed::ZERO;
        let live = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[live.index as usize].cur, fx(75), "the living target takes the hit");
        assert_eq!(world.last_attacker[live.index as usize], Some(shooter));
    }

    #[test]
    fn fire_breaks_ties_to_lowest_index() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Two equidistant on-axis enemies; the lowest slot index must take the shot.
        let low = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());
        let high = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[low.index as usize].cur, fx(75), "lowest-index target hit");
        assert_eq!(world.health[high.index as usize].cur, fx(100));
    }

    #[test]
    fn fire_never_hits_same_faction() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        let friendly = spawn_unit(&mut world, 5, 0, Faction::Player, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[friendly.index as usize].cur, fx(100), "no friendly fire");
        assert!(events.is_empty());
    }

    #[test]
    fn fire_kills_then_combat_death_pass_despawns() {
        // Command::Fire applies BEFORE combat_system in a tick: a lethal embodied shot drops the
        // target to 0 health, and the same tick's death pass emits Killed + despawns it.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 100, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(world.health[enemy.index as usize].is_dead());
        // Now run the auto-resolver's death pass (as Sim::step would, right after apply).
        let mut rng = Rng::new(1);
        combat_system(&mut world, &terrain, &mut rng, &mut events);
        assert!(!world.is_alive(enemy), "lethal embodied shot despawns the target this tick");
        assert!(events.iter().any(|e| matches!(e, SimEvent::Killed { .. })));
    }

    // --- Buildings are damageable / destroyable targets -------------------------------------

    #[test]
    fn embodied_fire_damages_then_destroys_enemy_building() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 50, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        // Enemy structure dead ahead on +X, distance 5 (< range 10), 100 HP.
        let building = spawn_building(&mut world, 5, 0, Faction::Enemy, 100);

        let mut events = Vec::new();
        // First shot: 50 dmg at full (open) multiplier — building survives, records the attacker.
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[building.index as usize].cur, fx(50), "building takes fire");
        assert_eq!(world.last_attacker[building.index as usize], Some(shooter));
        assert_eq!(events.len(), 1, "one Damaged event for the hit building");

        // Second shot drops it to zero; the death pass then despawns it (same as a unit).
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(world.health[building.index as usize].is_dead());
        let mut rng = Rng::new(1);
        combat_system(&mut world, &terrain, &mut rng, &mut events);
        assert!(!world.is_alive(building), "a razed building is despawned exactly like a unit");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SimEvent::Killed { entity, .. } if *entity == building)),
            "destruction emits a Killed event for the building"
        );
    }

    #[test]
    fn attack_moving_unit_razes_enemy_building() {
        // RTS auto-combat: a FireAtWill unit in range of an enemy building destroys it. The
        // AttackMove order documents the scenario (movement is resolved by `orders`, not here);
        // `combat_system` is what applies the damage and despawns the structure.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 50, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        world.order[shooter.index as usize] =
            crate::components::Order::AttackMove(Vec2::new(fx(5), fx(0)));
        let building = spawn_building(&mut world, 3, 0, Faction::Enemy, 100);

        let mut events = Vec::new();
        // Tick 1: 50 dmg lands on the enemy building.
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[building.index as usize].cur,
            fx(50),
            "a combat unit auto-targets the enemy building"
        );
        // Tick 2: the second 50 dmg razes it and the death pass despawns it the same tick.
        run(&mut world, &terrain, &mut events);
        assert!(!world.is_alive(building), "attack-moving unit razes the enemy building");
        assert!(events.iter().any(|e| matches!(e, SimEvent::Killed { .. })));
    }

    #[test]
    fn friendly_building_is_never_targeted() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 50, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // A friendly structure squarely inside the aim cone and in weapon range.
        let friendly = spawn_building(&mut world, 5, 0, Faction::Player, 100);

        let mut events = Vec::new();
        // Embodied fire must not hit an own-faction building (no friendly fire on the base).
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(
            world.health[friendly.index as usize].cur,
            fx(100),
            "embodied fire spares the friendly building"
        );
        // Auto-combat must not auto-target it either (the shooter goes order-driven for this pass).
        world.input_source[shooter.index as usize] = InputSource::Orders;
        for _ in 0..5 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(
            world.health[friendly.index as usize].cur,
            fx(100),
            "a FireAtWill unit never engages a friendly building"
        );
        assert!(events.is_empty(), "no damage events against a friendly building");
    }

    #[test]
    fn destroyed_building_stops_producing() {
        use crate::components::{Building, BuildingKind, ProductionItem, UnitKind};
        use crate::economy::{economy_system, Resources};
        use crate::territory::Territory;

        let mut world = World::new();
        let terrain = Terrain::open();
        // A built enemy camp with a Rifleman one tick from completion.
        let camp = spawn_building(&mut world, 5, 0, Faction::Enemy, 100);
        let ci = camp.index as usize;
        world.building[ci] = Building {
            kind: BuildingKind::Camp,
            level: 0,
            build_ticks_left: 0,
            queue: vec![ProductionItem {
                kind: UnitKind::Rifleman,
                ticks_left: 1,
            }],
        };

        // An embodied player razes the camp before the unit finishes.
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 100, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(world.health[ci].is_dead());
        let mut rng = Rng::new(1);
        combat_system(&mut world, &terrain, &mut rng, &mut events);
        assert!(!world.is_alive(camp), "razed camp is despawned");

        // The economy must skip the despawned camp: no unit spawns, no UnitProduced event.
        let mut resources = Resources::new(0);
        let territory = Territory::empty();
        let mut eco_events = Vec::new();
        economy_system(
            &mut world,
            &mut resources,
            &territory,
            &mut eco_events,
            &mut rng,
        );
        let units = (0..world.capacity())
            .filter(|&i| world.is_index_alive(i) && world.kind[i] == EntityKind::Unit)
            .count();
        assert_eq!(units, 1, "only the shooter remains; the razed camp produced nothing");
        assert!(
            !eco_events
                .iter()
                .any(|e| matches!(e, SimEvent::UnitProduced { .. })),
            "a destroyed camp emits no production"
        );
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

//! Fixed-point ballistic projectiles — the tank gun's shell flight (tank embodiment P3, D55).
//!
//! War Thunder's soul is **travel time** (you lead a moving target) and **drop** (the shell
//! arcs). This module adds both while keeping the world 2D (invariant #1, plan §6a): units stay
//! on the ground plane at a known hull height; only the shell carries its own vertical state
//! (`height` + `vz`) and integrates gravity. A finite [`Weapon::muzzle_vel`](crate::components::Weapon)
//! turns an embodied [`Fire`](crate::sim::Command::Fire) into a launched [`Projectile`] instead of
//! the instant [`combat::resolve_fire`](crate::combat::resolve_fire) hitscan; `muzzle_vel == 0`
//! (the infantry default) keeps the hitscan path untouched, so this system is **opt-in by a zero
//! default** and costs every existing unit nothing.
//!
//! ## Embodied-only, by construction (invariant #3)
//! Only an embodied unit's `Fire` reaches [`fire_ballistic`] (the AI auto-resolver
//! [`combat_system`](crate::combat::combat_system) is unchanged — it stays instant hitscan and
//! skips embodied units). Projectiles are therefore a first-person advantage, exactly like the
//! magazine and crouch (D51): the literal-executor AI never gains projectile micro.
//!
//! ## Determinism (invariants #1, #7)
//! Everything is `Fixed`/`Angle`/integer — no float, no transcendental. The pool is a plain
//! `Vec<Projectile>` advanced in **stable index order**; impact picks the **lowest-index** living
//! hostile (ties broken low, like `resolve_fire`); compaction retains in index order. The pool is
//! **folded into the per-tick checksum + serialized** ([`Sim::fold`](crate::sim)), so an in-flight
//! shell is part of the lockstep state. A hard [`MAX_PROJECTILES`] cap bounds the pool against the
//! Phase-3 thermal budget; an overflow shot is dropped deterministically.

use crate::combat::{is_enemy, SUPPRESSION_MAX, SUPPRESSION_PER_HIT};
use crate::components::{Faction, Vec2};
use crate::ecs::{Entity, World};
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::flow_field::HALF_EXTENT;
use crate::terrain::Terrain;

/// Hard cap on shells in flight (the bounded ring of plan §6a). A single embodied tank can never
/// approach this given its reload, so it is a thermal safety valve, not a gameplay limit: an
/// overflow shot is dropped deterministically rather than growing the pool unbounded in a
/// 200-unit firefight. A power of two for a tidy bound; pure count, no float.
pub const MAX_PROJECTILES: usize = 256;

/// Gravity pulling a shell's vertical velocity down each tick, in `Fixed` world-units-per-tick of
/// `vz` change (the `vz -= GRAVITY` of plan §6a). `1/256` per tick is a gentle, readable arc at
/// the locked 60 Hz: a flat-fired shell from [`MUZZLE_HEIGHT`] stays within a hull band over a
/// direct-fire distance and visibly drops over a long lob. Playtest baseline (dial once P7/P8 make
/// the arc visible); exact ratio keeps it float-free (invariant #1).
pub const GRAVITY: Fixed = Fixed::from_ratio(1, 256);

/// The height a shell launches at — the gun barrel above the ground plane, in world units. Sits
/// inside the hull band ([`HULL_HEIGHT`]) so a flat (level) shot at a target in range connects,
/// the way direct fire should. `1/2` of a world cell. Playtest baseline; exact ratio, no float.
pub const MUZZLE_HEIGHT: Fixed = Fixed::from_ratio(1, 2);

/// The shell's initial vertical velocity at the muzzle. **Flat fire** (`0`): the signature *drop*
/// then comes purely from [`GRAVITY`], so a direct shot never arcs up and over a near target. Kept
/// a named constant so an indirect-fire lob is a one-line change later. Float-free by construction.
pub const LAUNCH_VZ: Fixed = Fixed::ZERO;

/// Top of a unit's hull band, in world units. A shell impacts only while its `height` is within
/// `[0, HULL_HEIGHT]` — above it the shell flies over (relevant once shells can be lobbed), below
/// `0` it has hit the dirt (an undershoot). `1` world cell tall. Playtest baseline; exact, no float.
pub const HULL_HEIGHT: Fixed = Fixed::ONE;

/// Horizontal impact radius — a shell hits a unit/building whose footprint centre is within this
/// distance of the shell on the ground plane (squared compare, never a sqrt). `1` world unit (a
/// unit footprint). Playtest baseline; exact, no float (invariant #1).
pub const HIT_RADIUS: Fixed = Fixed::ONE;

/// Default shell lifetime in ticks — a hard despawn cap so a shell that hits nothing (cleared the
/// map, or flew over open ground) cannot live forever. `180` ticks is three seconds at the locked
/// 60 Hz, far longer than any shell needs to cross the `128`-wide playfield. Integer, no float.
pub const DEFAULT_LIFETIME: u16 = 180;

/// One in-flight shell. A plain fixed-point value (Copy), stored in the `Sim` pool and folded into
/// the per-tick checksum (invariant #7). Verticality is **localized here** (plan §6a): the world
/// stays 2D; only the shell carries `height`/`vz` and integrates gravity. No float anywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Projectile {
    /// Ground-plane position (world units). Integrated by `vel2d` each tick.
    pub pos2d: Vec2,
    /// Ground-plane velocity (world units per tick) — the aim direction scaled to `muzzle_vel`.
    /// Constant in flight (no horizontal drag); finite, so the shell has travel time.
    pub vel2d: Vec2,
    /// Height above the ground plane (world units). Drops under gravity; `< 0` means it hit dirt.
    pub height: Fixed,
    /// Vertical velocity (world units per tick). Decremented by gravity each tick (the arc/drop).
    pub vz: Fixed,
    /// The firing entity (for `last_attacker` + the `Damaged` event source). May go stale if the
    /// shooter despawns mid-flight — handled exactly as `combat`'s dead-attacker case.
    pub owner: Entity,
    /// The shooter's faction — carried so impact hostility (`is_enemy`) needs no live owner.
    pub faction: Faction,
    /// Damage on impact, before cover mitigation (mirrors `Weapon::damage`).
    pub damage: Fixed,
    /// Armour penetration carried for the P4 facing model (unused in P3). `0` until P4 wires it
    /// from `Weapon`; folded now so the pool's byte layout is stable across the P3→P4 boundary.
    pub penetration: Fixed,
    /// Ticks of flight remaining before a forced despawn ([`DEFAULT_LIFETIME`] at launch).
    pub lifetime: u16,
}

/// Faction tag for the fold (mirrors `sim`'s private encoder, kept local so the pool block is
/// self-contained). Stable repr — adding a faction is a compile error here.
#[inline]
pub(crate) fn faction_tag(f: Faction) -> u8 {
    match f {
        Faction::Player => 0,
        Faction::Enemy => 1,
        Faction::Neutral => 2,
    }
}

/// Is `(x, y)` outside the playfield? The grid covers `[-HALF_EXTENT, HALF_EXTENT)` on each axis
/// (terrain reads beyond it clamp to the border, so this is only a despawn trigger — a shell that
/// leaves the map is spent). Pure fixed-point compare.
#[inline]
fn out_of_bounds(p: Vec2) -> bool {
    p.x < -HALF_EXTENT || p.x >= HALF_EXTENT || p.y < -HALF_EXTENT || p.y >= HALF_EXTENT
}

/// Pick the lowest-index living hostile (unit OR building) the shell `p` impacts this tick, or
/// `None`. Impact requires the shell to be within the hull height band `[0, HULL_HEIGHT]` and
/// within [`HIT_RADIUS`] of the target footprint (squared compare — no sqrt). Scans in stable
/// index order; the first qualifier (lowest index) wins ties, exactly like `resolve_fire`.
fn find_impact(world: &World, p: &Projectile) -> Option<usize> {
    // Outside the vertical hull band: either flew over (height above) or already hit dirt (below).
    if p.height < Fixed::ZERO || p.height > HULL_HEIGHT {
        return None;
    }
    // A shell that has left the playfield can hit nothing — it is spent, and step 3 reclaims it.
    // Guarding here also keeps the squared-distance compare below inside the fixed-point range
    // where it cannot overflow: an in-map shell is always within bounded distance of an in-map
    // target, so `len_sq()` never wraps. Self-documenting: off the map means no impact.
    if out_of_bounds(p.pos2d) {
        return None;
    }
    let r_sq = HIT_RADIUS * HIT_RADIUS;
    for t in 0..world.capacity() {
        if !world.is_index_alive(t) {
            continue;
        }
        if world.health[t].is_dead() {
            continue;
        }
        // Hostile only — no friendly fire, no neutral hits (mirrors combat's `is_enemy`). A
        // building is a valid target (the same "no kind filter on the target" rule as resolve_fire).
        if !is_enemy(p.faction, world.faction[t]) {
            continue;
        }
        if (world.pos[t] - p.pos2d).len_sq() <= r_sq {
            return Some(t);
        }
    }
    None
}

/// Apply a shell's impact damage to `target_idx` — the **same** cover-mitigated write
/// `resolve_fire`/`combat_system` apply (terrain `cover_at` multiplier, set `last_attacker`, add
/// suppression, emit `Damaged`). Resolving here, at impact, is what makes a shell that catches a
/// unit mid-move hit where it ended up (and is the seam P4's armour facing slots into). Touches
/// only already-checksummed fields.
fn apply_impact(
    world: &mut World,
    terrain: &Terrain,
    p: &Projectile,
    target_idx: usize,
    events: &mut Vec<SimEvent>,
) {
    let target = match world.entity(target_idx) {
        Some(e) => e,
        None => return,
    };
    let mult = terrain.cover_at(world.pos[target_idx]).damage_multiplier();
    let damage = p.damage * mult;
    world.health[target_idx].cur -= damage;
    world.last_attacker[target_idx] = Some(p.owner);
    world.suppression[target_idx] =
        (world.suppression[target_idx] + SUPPRESSION_PER_HIT).min(SUPPRESSION_MAX);
    events.push(SimEvent::Damaged {
        entity: target,
        faction: world.faction[target_idx],
        source: p.owner,
        amount: damage,
        pos: world.pos[target_idx],
    });
}

/// Launch a ballistic shell for an embodied `Fire` along `dir`, mirroring `resolve_fire`'s pre-fire
/// gates and ammo/cooldown spend (tank embodiment P3, D55). Returns `true` iff a shell was spawned.
///
/// Gates (identical to the hitscan path so a tank gun obeys the same fire contract): a disarmed
/// (range 0) weapon, a hot cooldown, and an empty/reloading magazine are each a silent no-op. A
/// **zero `dir`** has no bearing → no shot (like `AimTurret`). When the pool is at
/// [`MAX_PROJECTILES`] the shot is **dropped deterministically** (no spawn, no spend) — the thermal
/// safety valve. On a successful launch the shell starts at the shooter's position and
/// [`MUZZLE_HEIGHT`], its `vel2d` is the unit aim scaled to `muzzle_vel`, and the weapon spends a
/// round + goes on cooldown exactly as `resolve_fire` does.
pub fn fire_ballistic(
    world: &mut World,
    shooter_idx: usize,
    dir: Vec2,
    pool: &mut Vec<Projectile>,
) -> bool {
    if !world.is_index_alive(shooter_idx) {
        return false;
    }
    let w = world.weapon[shooter_idx];
    // Same pre-fire gates as resolve_fire: armed, ready, and (if it has a magazine) loaded.
    if w.range <= Fixed::ZERO || w.cooldown_left != 0 {
        return false;
    }
    if w.mag_size > 0 && (w.reload_left > 0 || w.ammo == 0) {
        return false;
    }
    // A zero look-stick has no direction to launch along — a dry click, no spend (mirrors the
    // zero-dir no-op of AimTurret and resolve_fire's on-muzzle exclusion).
    let aim = dir.normalized();
    if aim == Vec2::ZERO {
        return false;
    }
    // Pool full: drop the overflow shot deterministically (no spawn, no ammo/cooldown spent).
    if pool.len() >= MAX_PROJECTILES {
        return false;
    }
    let owner = match world.entity(shooter_idx) {
        Some(e) => e,
        None => return false,
    };
    pool.push(Projectile {
        pos2d: world.pos[shooter_idx],
        vel2d: aim.scale(w.muzzle_vel),
        height: MUZZLE_HEIGHT,
        vz: LAUNCH_VZ,
        owner,
        faction: world.faction[shooter_idx],
        damage: w.damage,
        // P4 will carry Weapon.penetration here; until then a shell carries zero (unused).
        penetration: Fixed::ZERO,
        lifetime: DEFAULT_LIFETIME,
    });
    // Spend a round + go on cooldown, identically to resolve_fire (the gate above guarantees
    // `ammo > 0` for a magazine weapon, so the decrement never underflows).
    world.weapon[shooter_idx].cooldown_left = w.cooldown_ticks;
    if w.mag_size > 0 {
        world.weapon[shooter_idx].ammo -= 1;
    }
    true
}

/// Advance every in-flight shell one tick and resolve impacts (tank embodiment P3, D55). Runs in
/// [`Sim::step`](crate::sim::Sim::step)'s fixed order **after** `combat_system`. Per shell, in
/// stable pool order:
///
/// 1. integrate — `pos2d += vel2d`; `height += vz`; `vz -= gravity` (the travel + arc of plan §6a);
/// 2. **impact** the lowest-index living hostile within [`HIT_RADIUS`] and the hull height band →
///    apply the same cover-mitigated damage `resolve_fire` does, then remove the shell;
/// 3. otherwise remove the shell if it hit the dirt (`height < 0`), left the map, or its lifetime
///    expired; else keep it (lifetime counts down).
///
/// Survivors are compacted **in place, in index order** (a deterministic retain), so the pool stays
/// peer-identical (invariant #7). `gravity` is a parameter (normally [`GRAVITY`]) so a test can
/// force a steep drop.
pub fn projectile_system(
    world: &mut World,
    terrain: &Terrain,
    pool: &mut Vec<Projectile>,
    events: &mut Vec<SimEvent>,
    gravity: Fixed,
) {
    let mut write = 0usize;
    for read in 0..pool.len() {
        let mut p = pool[read];
        // 1. integrate flight (ground-plane travel, then the vertical arc/drop).
        p.pos2d = p.pos2d + p.vel2d;
        p.height += p.vz;
        p.vz -= gravity;
        // 2. impact resolves first (a shell inside the hull band on a target hits it).
        if let Some(target_idx) = find_impact(world, &p) {
            apply_impact(world, terrain, &p, target_idx, events);
            continue; // remove on impact
        }
        // 3. spent: hit the dirt, left the map, or timed out — each a silent despawn (no damage).
        if p.height < Fixed::ZERO || out_of_bounds(p.pos2d) || p.lifetime == 0 {
            continue;
        }
        p.lifetime -= 1;
        pool[write] = p;
        write += 1;
    }
    pool.truncate(write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Health, InputSource, Weapon};
    use crate::terrain::{Cover, Terrain};

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    /// Spawn a hostile target at `(x, y)` with `hp`, returning its slot index.
    fn spawn_target(world: &mut World, x: i32, y: i32, faction: Faction, hp: i32) -> usize {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.health[i] = Health::full(fx(hp));
        i
    }

    /// A bare shell heading +X at `speed`/tick from `(x, y)`, owned by a synthetic player handle,
    /// at the muzzle height with no vertical launch (flat fire — drop is gravity-only).
    fn shell(x: i32, y: i32, speed: Fixed, damage: i32) -> Projectile {
        Projectile {
            pos2d: Vec2::new(fx(x), fx(y)),
            vel2d: Vec2::new(speed, Fixed::ZERO),
            height: MUZZLE_HEIGHT,
            vz: LAUNCH_VZ,
            owner: Entity { index: 0, generation: 0 },
            faction: Faction::Player,
            damage: fx(damage),
            penetration: Fixed::ZERO,
            lifetime: DEFAULT_LIFETIME,
        }
    }

    fn run(world: &mut World, pool: &mut Vec<Projectile>, events: &mut Vec<SimEvent>, gravity: Fixed) {
        let terrain = Terrain::open();
        projectile_system(world, &terrain, pool, events, gravity);
    }

    #[test]
    fn shell_travels_at_muzzle_velocity_and_takes_travel_time() {
        // A shell at 2 units/tick reaches a target 10 away in ~5 ticks (NOT instantly — this is the
        // whole point of finite muzzle velocity). A near target is reached in fewer ticks.
        let mut world = World::new();
        let far = spawn_target(&mut world, 10, 0, Faction::Enemy, 100);
        let mut pool = vec![shell(0, 0, fx(2), 25)];
        let mut events = Vec::new();
        // No gravity in this test so the only variable is horizontal travel time.
        let mut ticks = 0;
        while !pool.is_empty() && ticks < 100 {
            run(&mut world, &mut pool, &mut events, Fixed::ZERO);
            ticks += 1;
        }
        assert!(world.health[far].cur < fx(100), "the shell eventually hits");
        assert_eq!(ticks, 5, "10 / 2 = 5 ticks of travel before impact");

        // A nearer target (distance 6) is hit sooner (3 ticks).
        let mut world2 = World::new();
        let near = spawn_target(&mut world2, 6, 0, Faction::Enemy, 100);
        let mut pool2 = vec![shell(0, 0, fx(2), 25)];
        let mut ev2 = Vec::new();
        let mut ticks2 = 0;
        while !pool2.is_empty() && ticks2 < 100 {
            run(&mut world2, &mut pool2, &mut ev2, Fixed::ZERO);
            ticks2 += 1;
        }
        assert!(world2.health[near].cur < fx(100));
        assert_eq!(ticks2, 3, "6 / 2 = 3 ticks");
        assert!(ticks2 < ticks, "the nearer target is hit sooner than the far one");
    }

    #[test]
    fn shell_arcs_down_under_gravity() {
        // With gravity the shell's height is non-increasing and nets a real drop (flat launch from
        // the muzzle height → pure gravity arc). The first tick holds (vz starts at 0), then each
        // subsequent tick falls. No target, so it survives until it hits the dirt.
        let mut world = World::new();
        let start = MUZZLE_HEIGHT;
        let mut pool = vec![shell(0, 0, fx(1), 25)];
        let mut events = Vec::new();
        let mut prev = start;
        for _ in 0..8 {
            run(&mut world, &mut pool, &mut events, GRAVITY);
            if pool.is_empty() {
                break;
            }
            assert!(pool[0].height <= prev, "height never rises under gravity (flat launch)");
            prev = pool[0].height;
        }
        assert!(prev < start, "the shell has dropped below its muzzle height");
    }

    #[test]
    fn undershoot_hits_the_ground_and_despawns_with_no_damage() {
        // A steep gravity drops the shell below ground (height < 0) before it reaches a far target:
        // it despawns as a dirt hit — no damage, no Damaged event.
        let mut world = World::new();
        let target = spawn_target(&mut world, 30, 0, Faction::Enemy, 100);
        let mut pool = vec![shell(0, 0, fx(1), 25)];
        let mut events = Vec::new();
        // Gravity steep enough that height < 0 within a couple ticks, well short of x = 30.
        let steep = Fixed::from_ratio(1, 2);
        while !pool.is_empty() {
            run(&mut world, &mut pool, &mut events, steep);
        }
        assert_eq!(world.health[target].cur, fx(100), "undershoot deals no damage");
        assert!(events.is_empty(), "a dirt hit emits no Damaged event");
    }

    #[test]
    fn impact_applies_cover_mitigated_damage_and_emits_damaged() {
        // Open terrain → full damage; the impact records last_attacker + emits one Damaged event.
        let mut world = World::new();
        let target = spawn_target(&mut world, 2, 0, Faction::Enemy, 100);
        let owner = world.spawn(); // a real owner handle the impact attributes to
        let mut pool = vec![Projectile { owner, ..shell(0, 0, fx(2), 25) }];
        let mut events = Vec::new();
        run(&mut world, &mut pool, &mut events, Fixed::ZERO);
        assert_eq!(world.health[target].cur, fx(75), "open terrain = full 25 damage");
        assert_eq!(world.last_attacker[target], Some(owner));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SimEvent::Damaged { amount, .. } if amount == fx(25)));
        assert!(pool.is_empty(), "the shell is consumed on impact");
    }

    #[test]
    fn impact_is_cover_mitigated() {
        // Heavy cover on the target's cell quarters the damage (the same multiplier resolve_fire
        // uses) — proving impact runs the unified cover-mitigated damage path.
        let mut world = World::new();
        let target = spawn_target(&mut world, 2, 0, Faction::Enemy, 100);
        let mut terrain = Terrain::open();
        let (cx, cy) = terrain.cell_of(Vec2::new(fx(2), fx(0)));
        terrain.set_cover(cx, cy, Cover::Heavy);
        let mut pool = vec![shell(0, 0, fx(2), 40)];
        let mut events = Vec::new();
        projectile_system(&mut world, &terrain, &mut pool, &mut events, Fixed::ZERO);
        // 40 * 1/4 = 10 damage through heavy cover.
        assert_eq!(world.health[target].cur, fx(90));
    }

    #[test]
    fn shell_flies_over_a_target_above_the_hull_band() {
        // A shell whose height is above the hull band overflies a target sitting in its footprint
        // without hitting it (the `height > HULL_HEIGHT` branch of find_impact). Flat (no gravity)
        // so it stays above the band this tick: the target is untouched and the shell flies on.
        let mut world = World::new();
        let target = spawn_target(&mut world, 2, 0, Faction::Enemy, 100);
        // Above the band, travelling +X to land exactly on the target's cell after one tick.
        let mut pool = vec![Projectile { height: HULL_HEIGHT + Fixed::ONE, ..shell(0, 0, fx(2), 25) }];
        let mut events = Vec::new();
        run(&mut world, &mut pool, &mut events, Fixed::ZERO);
        assert_eq!(world.health[target].cur, fx(100), "an overflying shell deals no damage");
        assert_eq!(pool.len(), 1, "the shell flew over, not consumed");
        assert!(events.is_empty(), "no impact, no Damaged event");
    }

    #[test]
    fn shell_passes_through_a_dead_target_without_damage_or_event() {
        // A target already at zero health occupies the impact footprint, but find_impact skips dead
        // entities — the shell passes through (no damage write, no Damaged event) and flies on. If
        // the is_dead() skip were removed this would spuriously "hit" a corpse.
        let mut world = World::new();
        let dead = spawn_target(&mut world, 2, 0, Faction::Enemy, 100);
        world.health[dead].cur = Fixed::ZERO; // already dead
        let mut pool = vec![shell(0, 0, fx(2), 25)];
        let mut events = Vec::new();
        run(&mut world, &mut pool, &mut events, Fixed::ZERO);
        assert_eq!(world.health[dead].cur, Fixed::ZERO, "a dead target takes no further damage");
        assert!(events.is_empty(), "no Damaged event for a dead target");
        assert_eq!(pool.len(), 1, "the shell passes through and stays in flight");
    }

    #[test]
    fn impact_breaks_ties_to_the_lowest_index() {
        // Two hostiles on the same cell — the lowest slot index takes the hit (stable order, like
        // resolve_fire's tie-break).
        let mut world = World::new();
        let low = spawn_target(&mut world, 2, 0, Faction::Enemy, 100);
        let high = spawn_target(&mut world, 2, 0, Faction::Enemy, 100);
        assert!(low < high);
        let mut pool = vec![shell(0, 0, fx(2), 25)];
        let mut events = Vec::new();
        run(&mut world, &mut pool, &mut events, Fixed::ZERO);
        assert_eq!(world.health[low].cur, fx(75), "lowest index hit");
        assert_eq!(world.health[high].cur, fx(100));
    }

    #[test]
    fn shell_spares_friendly_and_neutral_targets() {
        // A player shell passes through a friendly and a neutral unit on its path (is_enemy gate),
        // and only damages the enemy beyond them.
        let mut world = World::new();
        let friend = spawn_target(&mut world, 2, 0, Faction::Player, 100);
        let neutral = spawn_target(&mut world, 2, 0, Faction::Neutral, 100);
        let mut pool = vec![shell(0, 0, fx(2), 25)];
        let mut events = Vec::new();
        run(&mut world, &mut pool, &mut events, Fixed::ZERO);
        assert_eq!(world.health[friend].cur, fx(100), "no friendly fire");
        assert_eq!(world.health[neutral].cur, fx(100), "no neutral hits");
        assert!(events.is_empty(), "nothing hostile in radius → no impact");
        // The shell flew on (not consumed by a friendly).
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn shell_despawns_on_lifetime_expiry() {
        // A shell that hits nothing dies when its lifetime runs out (the hard cap), not forever.
        let mut world = World::new();
        let mut pool = vec![Projectile { lifetime: 3, ..shell(0, 0, fx(1), 25) }];
        let mut events = Vec::new();
        for _ in 0..3 {
            run(&mut world, &mut pool, &mut events, Fixed::ZERO);
            assert!(!pool.is_empty(), "alive while lifetime remains");
        }
        run(&mut world, &mut pool, &mut events, Fixed::ZERO);
        assert!(pool.is_empty(), "despawns once lifetime hits zero");
    }

    #[test]
    fn shell_despawns_when_leaving_the_map() {
        // A shell fired toward the border leaves the playfield and is reclaimed (no infinite flight
        // off the map). Start near the +X edge.
        let mut world = World::new();
        let mut pool = vec![shell(60, 0, fx(4), 25)];
        let mut events = Vec::new();
        let mut ticks = 0;
        while !pool.is_empty() && ticks < 20 {
            run(&mut world, &mut pool, &mut events, Fixed::ZERO);
            ticks += 1;
        }
        assert!(pool.is_empty(), "the shell despawns after crossing the map edge");
        assert!(ticks <= 2, "it leaves quickly from near the border");
    }

    #[test]
    fn fire_ballistic_spawns_a_shell_and_spends_ammo_and_cooldown() {
        // The embodied fire seam: a ballistic gun spawns a shell along the aim, scaled to muzzle_vel,
        // and spends a round + sets cooldown exactly like resolve_fire.
        let mut world = World::new();
        let e = world.spawn();
        let i = e.index as usize;
        world.input_source[i] = InputSource::Embodied;
        world.weapon[i] = Weapon {
            range: fx(20),
            damage: fx(50),
            cooldown_ticks: 12,
            cooldown_left: 0,
            mag_size: 3,
            ammo: 3,
            reload_ticks: 60,
            reload_left: 0,
            turret_speed: 100,
            muzzle_vel: fx(2),
        };
        let mut pool = Vec::new();
        let fired = fire_ballistic(&mut world, i, Vec2::new(Fixed::ONE, Fixed::ZERO), &mut pool);
        assert!(fired);
        assert_eq!(pool.len(), 1);
        assert_eq!(pool[0].vel2d, Vec2::new(fx(2), Fixed::ZERO), "aim scaled to muzzle_vel");
        assert_eq!(pool[0].height, MUZZLE_HEIGHT);
        assert_eq!(world.weapon[i].ammo, 2, "one round spent");
        assert_eq!(world.weapon[i].cooldown_left, 12, "cooldown set");
    }

    #[test]
    fn fire_ballistic_respects_cooldown_empty_mag_and_zero_dir() {
        let mut world = World::new();
        let e = world.spawn();
        let i = e.index as usize;
        world.weapon[i] = Weapon {
            range: fx(20),
            damage: fx(50),
            cooldown_ticks: 12,
            cooldown_left: 5, // hot
            mag_size: 3,
            ammo: 3,
            reload_ticks: 60,
            reload_left: 0,
            turret_speed: 100,
            muzzle_vel: fx(2),
        };
        let mut pool = Vec::new();
        let east = Vec2::new(Fixed::ONE, Fixed::ZERO);
        assert!(!fire_ballistic(&mut world, i, east, &mut pool), "no fire while hot");
        // Cooldown clear but empty mag → still no fire.
        world.weapon[i].cooldown_left = 0;
        world.weapon[i].ammo = 0;
        assert!(!fire_ballistic(&mut world, i, east, &mut pool), "empty mag dry-clicks");
        // Mid-reload (ammo spent, reload_left counting down): a magazine weapon that is currently
        // reloading dry-clicks too — the same gate resolve_fire applies — no shell, no spend.
        world.weapon[i].reload_left = 10;
        assert!(!fire_ballistic(&mut world, i, east, &mut pool), "mid-reload dry-clicks");
        assert_eq!(world.weapon[i].ammo, 0, "a mid-reload click spends nothing");
        assert!(pool.is_empty(), "no shell spawned mid-reload");
        world.weapon[i].reload_left = 0;
        // Reload it; a zero dir has no bearing → still no fire (and no spend).
        world.weapon[i].ammo = 3;
        assert!(!fire_ballistic(&mut world, i, Vec2::ZERO, &mut pool), "zero aim → no shot");
        assert_eq!(world.weapon[i].ammo, 3, "a dry/zero click spends nothing");
        assert!(pool.is_empty());
    }

    #[test]
    fn pool_ring_cap_drops_the_overflow_shot_deterministically() {
        // With the pool already at MAX, a fire is dropped: no spawn, no ammo/cooldown spent. Two
        // independent runs make the identical decision (deterministic).
        fn attempt() -> (bool, usize, u16) {
            let mut world = World::new();
            let e = world.spawn();
            let i = e.index as usize;
            world.weapon[i] = Weapon {
                range: fx(20),
                damage: fx(50),
                cooldown_ticks: 12,
                cooldown_left: 0,
                mag_size: 5,
                ammo: 5,
                reload_ticks: 60,
                reload_left: 0,
                turret_speed: 0,
                muzzle_vel: fx(2),
            };
            // Fill the pool to the hard cap.
            let mut pool: Vec<Projectile> = (0..MAX_PROJECTILES)
                .map(|_| shell(0, 0, fx(2), 10))
                .collect();
            let fired = fire_ballistic(&mut world, i, Vec2::new(Fixed::ONE, Fixed::ZERO), &mut pool);
            (fired, pool.len(), world.weapon[i].ammo)
        }
        let (fired, len, ammo) = attempt();
        assert!(!fired, "the overflow shot is dropped");
        assert_eq!(len, MAX_PROJECTILES, "pool never exceeds the cap");
        assert_eq!(ammo, 5, "a dropped shot spends no ammo");
        assert_eq!(attempt(), (false, MAX_PROJECTILES, 5), "the drop is deterministic");
    }
}

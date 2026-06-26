//! Debug / validation scenes — the "debug versions" that load a tiny, fully-deterministic world
//! to exercise ONE mechanic in isolation and prove it works.
//!
//! The point of this module is **single-sourcing**: a scene is seeded the same way on every
//! surface, so the thing you *watch* is the thing CI *checks*. The headless [`sim-runner`] seeds a
//! scene, drives a scripted input, and reports / asserts the outcome; the desktop `app` and the
//! offscreen `viz-runner` seed the **identical** `Sim` and render it, so a screenshot corresponds
//! to an assertion. Because the seeder is pure `core` (invariant #1/#2: fixed-point, no platform
//! deps), that correspondence is bit-exact across devices — there is no second, drifting copy of
//! the scene living in a host.
//!
//! ## The tank duel ([`seed_duel`])
//!
//! The first scene is a two-tank hitbox duel: two armoured chassis facing off along the X axis,
//! each with a ballistic direct-fire main gun. It exists to validate the **all-unit armour-facet +
//! ballistic-shell** model (D55) — the hitbox / penetration / facing code that already ships in
//! [`combat`](crate::combat) / [`projectile`](crate::projectile) but isn't yet carried by any
//! *produced* unit. Per the prototyping call, it reuses the existing [`UnitKind::Heavy`] chassis
//! rather than introducing a new `Tank` kind: the scene layers tank-like [`Armor`] + a
//! `muzzle_vel`/`penetration` gun onto that chassis **locally**, touching neither
//! [`economy::unit_stats`](crate::economy::unit_stats) nor the shipping balance.
//!
//! The numbers are chosen so the mechanic reads at a glance — *angle the hull, flank to kill*:
//! the gun's penetration cleanly **bounces** off the thick frontal facet but **pens** the thinner
//! side and rear (see [`DUEL_GUN_PENETRATION`]). A head-on exchange therefore goes nowhere; you
//! have to manoeuvre onto a flank. That is exactly the assertion the harness pins down and exactly
//! the lesson the playable sandbox teaches.

use crate::components::{Armor, EntityKind, Faction, Health, Stance, UnitKind, Vec2, Weapon};
use crate::ecs::Entity;
use crate::economy;
use crate::fixed::Fixed;
use crate::sim::Sim;
use crate::terrain::Cover;
use crate::trig::{Angle, ANGLE_FULL};

/// Half the gap between the two duelling tanks: each sits this far from the origin on the X axis,
/// facing the other. `6` world units → a 12-unit no-man's-land the shells cross in a few ticks at
/// [`DUEL_GUN_MUZZLE_VEL`], close enough to read on screen.
pub const DUEL_HALF_SPACING: i32 = 6;

/// Starting hit points of a duel tank. Sized (with [`DUEL_GUN_DAMAGE`]) so a kill takes a couple
/// of *penetrating* hits — enough ticks to watch the shells fly, few enough that the report is
/// short.
pub const DUEL_TANK_HP: Fixed = Fixed::from_int(200);

/// Frontal armour — the thickest facet. Chosen so [`DUEL_GUN_PENETRATION`] cannot crack it head-on
/// (a clean bounce: `2·18 ≤ 40` ⇒ `0×` damage in
/// [`facing_penetration_multiplier`](crate::combat::facing_penetration_multiplier)).
pub const DUEL_ARMOR_FRONT: Fixed = Fixed::from_int(40);
/// Flank armour — thin enough that the gun pens it cleanly (`18 ≥ 16` ⇒ full damage).
pub const DUEL_ARMOR_SIDE: Fixed = Fixed::from_int(16);
/// Tail armour — the thinnest facet (`18 ≥ 8` ⇒ full damage).
pub const DUEL_ARMOR_REAR: Fixed = Fixed::from_int(8);

/// The duel gun's reach. Far larger than [`DUEL_HALF_SPACING`] so distance never gates the fight —
/// the *facet*, not the range, decides every shot.
pub const DUEL_GUN_RANGE: Fixed = Fixed::from_int(40);
/// Damage per shot, *before* cover + the facet multiplier. With [`DUEL_TANK_HP`] this is two clean
/// pens to kill.
pub const DUEL_GUN_DAMAGE: Fixed = Fixed::from_int(120);
/// Ticks between shots — half a second at the locked 60 Hz.
pub const DUEL_GUN_COOLDOWN: u16 = 30;
/// Shell muzzle velocity (world units / tick). Non-zero ⇒ the gun is **ballistic**
/// ([`projectile`](crate::projectile)), not hitscan: a shot becomes a travelling shell you can
/// watch cross the gap and that resolves its facet on impact. `2` clears the 12-unit gap in ~6
/// ticks, well inside the shell's gravity-limited life.
pub const DUEL_GUN_MUZZLE_VEL: Fixed = Fixed::from_int(2);
/// Armour penetration the shell carries. The hinge of the whole scene: `18` sits **below** the
/// `40` front facet (so `2·18 ≤ 40` ⇒ a hard bounce) but **at or above** the `16` side and `8`
/// rear (so `18 ≥` both ⇒ full damage). Front-on you bounce; flank or rear you kill.
pub const DUEL_GUN_PENETRATION: Fixed = Fixed::from_int(18);

/// The handles a seeded duel hands back, so a harness / host can drive and inspect the two tanks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Duel {
    /// The left-hand tank at `(-DUEL_HALF_SPACING, 0)`, facing `+X` (`hull_heading == 0`). The one
    /// the playable sandbox embodies.
    pub player: Entity,
    /// The right-hand tank at `(+DUEL_HALF_SPACING, 0)`, facing `−X` (toward the player).
    pub enemy: Entity,
}

/// The shared duel-tank main gun: a ballistic, magazine-less direct-fire gun. No magazine
/// (`mag_size == 0`) keeps the harness/sandbox free of a reload gate — the focus is armour facing,
/// not ammo. The turret is locked to the hull (`turret_speed == 0`): independent turret aiming is
/// a later layer; here the chassis *is* the gun line, which is what makes hull angling the whole
/// game.
fn duel_gun() -> Weapon {
    Weapon {
        range: DUEL_GUN_RANGE,
        damage: DUEL_GUN_DAMAGE,
        cooldown_ticks: DUEL_GUN_COOLDOWN,
        cooldown_left: 0,
        mag_size: 0,
        ammo: 0,
        reload_ticks: 0,
        reload_left: 0,
        turret_speed: 0,
        muzzle_vel: DUEL_GUN_MUZZLE_VEL,
        penetration: DUEL_GUN_PENETRATION,
    }
}

/// Tank-like directional armour for a duel tank (thick front, thin sides, thinnest rear).
fn duel_armor() -> Armor {
    Armor {
        front: DUEL_ARMOR_FRONT,
        side: DUEL_ARMOR_SIDE,
        rear: DUEL_ARMOR_REAR,
    }
}

/// Spawn one duel tank: a [`Heavy`](UnitKind::Heavy) chassis re-dressed locally with tank armour +
/// the ballistic [`duel_gun`], pointed along `hull_heading`, holding fire (so only scripted /
/// embodied shots happen — no auto-fire noise in the report). Returns its handle.
fn spawn_duel_tank(sim: &mut Sim, pos: Vec2, faction: Faction, hull_heading: Angle) -> Entity {
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.unit_kind[i] = UnitKind::Heavy;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = pos;
    sim.world.health[i] = Health::full(DUEL_TANK_HP);
    sim.world.weapon[i] = duel_gun();
    sim.world.armor[i] = duel_armor();
    // HoldFire: the literal-executor AI never opens up on its own (invariant #3) — every shot in
    // this scene is one the harness scripts or the embodied player pulls, so the validation only
    // sees the shots it asked for.
    sim.world.stance[i] = Stance::HoldFire;
    sim.world.hull_heading[i] = hull_heading;
    sim.world.turret_yaw[i] = hull_heading;
    e
}

/// Seed `sim` with the two-tank hitbox duel and return the [`Duel`] handles. Player on the left
/// facing `+X`, enemy on the right facing `−X` (toward the player) — a head-on stand-off whose
/// frontal armour both guns bounce off, so progress means manoeuvring onto a flank.
///
/// Pure, deterministic, fixed-point: spawn order is stable and every value is integer / `Fixed`,
/// so two seeds of the same fresh `Sim` are bit-identical (invariant #1) — the property the
/// `app`/`viz-runner`/`sim-runner` correspondence rests on.
pub fn seed_duel(sim: &mut Sim) -> Duel {
    let d = Fixed::from_int(DUEL_HALF_SPACING);
    let player = spawn_duel_tank(
        sim,
        Vec2::new(-d, Fixed::ZERO),
        Faction::Player,
        Angle(0), // +X: facing the enemy
    );
    let enemy = spawn_duel_tank(
        sim,
        Vec2::new(d, Fixed::ZERO),
        Faction::Enemy,
        Angle(ANGLE_FULL / 2), // −X: facing the player
    );
    Duel { player, enemy }
}

// --- The infantry scene -------------------------------------------------------------------------

/// Max HP of a debug enemy rifleman — deliberately **low** (a produced Rifleman is 100) so the
/// sandbox fight resolves in a handful of shots and the harness report stays short. Debug-scene
/// local; it touches no shipping stat.
pub const INF_ENEMY_HP: Fixed = Fixed::from_int(12);
/// The player rifleman keeps a full produced-Rifleman HP pool so it survives the demonstration.
pub const INF_PLAYER_HP: Fixed = Fixed::from_int(100);

/// Enemy placements, with the player embodied at the origin facing `+X`. Each one isolates a
/// different infantry mechanic — the `+X`-aiming player engages them in this order
/// (`resolve_fire` takes the lowest-index target inside cone + range + LoS):
/// - **open** — on-axis, open ground, mid-range: the clean kill (full damage).
/// - **cover** — in **Light** cover, just inside the standing cone: same shot, **half** damage.
/// - **walled** — in cone + range but behind a **Heavy** wall: **line-of-sight** blocks the shot.
/// - **far** — on-axis but **beyond base range** (16 > 14): unreachable until the player **crouches**
///   (range ×5/4 = 17.5), the crouch range bonus made visible.
/// - **flank** — in range but **outside the standing cone** (~63° off `+X`): the cone made visible.
pub const INF_OPEN: (i32, i32) = (8, 0);
pub const INF_COVER: (i32, i32) = (9, 3);
pub const INF_WALLED: (i32, i32) = (10, -4);
pub const INF_FAR: (i32, i32) = (16, 1);
pub const INF_FLANK: (i32, i32) = (4, 8);

/// The handles a seeded infantry scene hands back. `player` is the embodiable Player rifleman; the
/// rest are HoldFire Enemy **dummies** (they never shoot back, so the player methodically eliminates
/// them and the validation only sees the player's own shots — invariant #3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Infantry {
    pub player: Entity,
    pub open: Entity,
    pub cover: Entity,
    pub walled: Entity,
    pub far: Entity,
    pub flank: Entity,
}

/// A world point from an `(i32, i32)` cell-aligned coordinate.
fn at(p: (i32, i32)) -> Vec2 {
    Vec2::new(Fixed::from_int(p.0), Fixed::from_int(p.1))
}

/// Spawn a [`Rifleman`](UnitKind::Rifleman) with the produced weapon loadout (range/cone/cover/LoS
/// all read against the real `economy::unit_stats` values) but an explicit `hp` (the scene uses a
/// low enemy HP for a short fight) and `stance`, facing `hull`.
fn spawn_rifleman(
    sim: &mut Sim,
    pos: Vec2,
    faction: Faction,
    stance: Stance,
    hp: Fixed,
    hull: Angle,
) -> Entity {
    let (_default_hp, weapon) = economy::unit_stats(UnitKind::Rifleman);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.unit_kind[i] = UnitKind::Rifleman;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = pos;
    sim.world.health[i] = Health::full(hp);
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = stance;
    sim.world.hull_heading[i] = hull;
    sim.world.turret_yaw[i] = hull;
    e
}

/// Seed `sim` with the infantry sandbox and return the [`Infantry`] handles: a Player rifleman at
/// the origin facing `+X`, and five Enemy **dummy** riflemen (HoldFire) positioned to isolate one
/// hitscan mechanic each (see [`INF_OPEN`]…[`INF_FLANK`]). Light cover sits on the **cover** dummy
/// and a Heavy wall blocks line of sight to the **walled** dummy.
///
/// Pure, deterministic, fixed-point — the same scene the headless `sim-runner infantry` harness
/// drives and the `app --scene infantry` sandbox renders (single-sourced like [`seed_duel`]).
pub fn seed_infantry(sim: &mut Sim) -> Infantry {
    let facing_enemy = Angle(ANGLE_FULL / 2); // dummies face −X, toward the player (cosmetic)
    let player = spawn_rifleman(
        sim,
        at((0, 0)),
        Faction::Player,
        Stance::FireAtWill,
        INF_PLAYER_HP,
        Angle(0), // +X, at the enemy line
    );
    let enemy = |sim: &mut Sim, p| {
        spawn_rifleman(sim, at(p), Faction::Enemy, Stance::HoldFire, INF_ENEMY_HP, facing_enemy)
    };
    let open = enemy(sim, INF_OPEN);
    let cover = enemy(sim, INF_COVER);
    let walled = enemy(sim, INF_WALLED);
    let far = enemy(sim, INF_FAR);
    let flank = enemy(sim, INF_FLANK);

    // Light cover on the `cover` dummy → its incoming damage is halved (`Cover::Light` = 1/2).
    let (ccx, ccy) = sim.terrain.cell_of(at(INF_COVER));
    sim.terrain.set_cover(ccx, ccy, Cover::Light);
    // A short Heavy wall straddling the sightline from the player (origin) to `walled` (10, −4):
    // the line passes ~(5, −2), so a 1×3 vertical Heavy bar there blocks LoS (Heavy blocks sight).
    let (wx, wy) = sim.terrain.cell_of(at((5, -2)));
    sim.terrain.fill_rect(wx, wy - 1, wx, wy + 1, Cover::Heavy);

    Infantry {
        player,
        open,
        cover,
        walled,
        far,
        flank,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::{facing_penetration_multiplier, shot_facet, Facet};
    use crate::sim::Command;

    fn fresh() -> Sim {
        Sim::new(0xD0E1)
    }

    /// The aim vector for a `+X`-travelling shot (player → enemy), as a unit `Fixed` vector.
    fn plus_x() -> Vec2 {
        Vec2::new(Fixed::ONE, Fixed::ZERO)
    }

    #[test]
    fn seeds_two_facing_tanks() {
        let mut sim = fresh();
        let duel = seed_duel(&mut sim);

        let p = duel.player.index as usize;
        let e = duel.enemy.index as usize;
        assert_eq!(sim.world.faction[p], Faction::Player);
        assert_eq!(sim.world.faction[e], Faction::Enemy);
        assert_eq!(sim.world.unit_kind[p], UnitKind::Heavy);
        // Facing: player looks +X (heading 0), enemy looks −X (half turn) — i.e. at each other.
        assert_eq!(sim.world.hull_heading[p], Angle(0));
        assert_eq!(sim.world.hull_heading[e], Angle(ANGLE_FULL / 2));
        // Placed symmetrically about the origin on the X axis.
        assert_eq!(sim.world.pos[p].x, Fixed::from_int(-DUEL_HALF_SPACING));
        assert_eq!(sim.world.pos[e].x, Fixed::from_int(DUEL_HALF_SPACING));
        assert_eq!(sim.world.pos[p].y, Fixed::ZERO);
    }

    #[test]
    fn both_tanks_carry_a_ballistic_armoured_loadout() {
        let mut sim = fresh();
        let duel = seed_duel(&mut sim);
        for idx in [duel.player.index as usize, duel.enemy.index as usize] {
            let w = sim.world.weapon[idx];
            assert!(w.muzzle_vel > Fixed::ZERO, "the gun is ballistic, not hitscan");
            assert_eq!(w.penetration, DUEL_GUN_PENETRATION);
            assert!(!sim.world.armor[idx].is_unarmored(), "the tank is armoured");
            // HoldFire so the literal-executor AI never auto-fires (invariant #3) — the harness
            // owns every shot.
            assert_eq!(sim.world.stance[idx], Stance::HoldFire);
        }
    }

    /// The load-bearing hitbox property: a head-on shot bounces, a flank/rear shot pens. This is
    /// the exact threshold the playable duel demonstrates, pinned to the seeded armour + gun.
    #[test]
    fn front_bounces_while_flank_and_rear_penetrate() {
        let mut sim = fresh();
        let duel = seed_duel(&mut sim);
        let e = duel.enemy.index as usize;
        let armor = sim.world.armor[e];
        let hull = sim.world.hull_heading[e]; // enemy faces −X
        let pen = sim.world.weapon[duel.player.index as usize].penetration;

        // A +X shot from the player strikes the enemy head-on → Front facet → clean bounce (0×).
        assert_eq!(shot_facet(plus_x(), hull), Facet::Front);
        assert_eq!(
            facing_penetration_multiplier(plus_x(), hull, pen, armor),
            Fixed::ZERO,
            "the gun bounces off the frontal facet — angle the hull / flank to kill",
        );

        // A shot arriving along +Y catches the flank → Side facet → full damage (1×).
        let from_flank = Vec2::new(Fixed::ZERO, Fixed::ONE);
        assert_eq!(shot_facet(from_flank, hull), Facet::Side);
        assert_eq!(
            facing_penetration_multiplier(from_flank, hull, pen, armor),
            Fixed::ONE,
            "the gun pens the thinner flank facet",
        );

        // A shot travelling −X (chasing the enemy's facing) catches the tail → Rear facet → full.
        let from_rear = Vec2::new(-Fixed::ONE, Fixed::ZERO);
        assert_eq!(shot_facet(from_rear, hull), Facet::Rear);
        assert_eq!(
            facing_penetration_multiplier(from_rear, hull, pen, armor),
            Fixed::ONE,
            "the gun pens the thinnest rear facet",
        );
    }

    /// Determinism: seeding the same fresh `Sim` twice yields a bit-identical world (invariant #1).
    /// This is what lets the headless harness and the rendered sandbox be the *same* scene.
    #[test]
    fn seeding_is_deterministic() {
        let mut a = fresh();
        let mut b = fresh();
        seed_duel(&mut a);
        seed_duel(&mut b);
        assert_eq!(a.checksum(), b.checksum());
    }

    /// Drive the duel through the real ballistic + armour pipeline for `ticks` ticks and return the
    /// final checksum. Embodies the player, exposes the enemy's flank from the start (so every shell
    /// *penetrates* — exercising `apply_impact`'s damage write, not just a bounce), and fires `+X` on
    /// the gun's cooldown cadence. This is the chain `Fire → fire_ballistic → projectile_system →
    /// apply_impact(facing_penetration_multiplier)` running through real `Sim::step` ticks.
    fn run_ballistic_duel(ticks: u64) -> u64 {
        let mut sim = fresh();
        let duel = seed_duel(&mut sim);
        // Expose the flank from the outset: a +X shell now strikes the Side facet and pens.
        sim.world.hull_heading[duel.enemy.index as usize] = Angle(ANGLE_FULL / 4);
        for tick in 1..ticks {
            let mut cmds: Vec<Command> = Vec::new();
            if tick == 1 {
                cmds.push(Command::Embody { entity: duel.player });
            } else if (tick - 2).is_multiple_of(DUEL_GUN_COOLDOWN as u64) {
                cmds.push(Command::Fire {
                    entity: duel.player,
                    dir: plus_x(),
                });
            }
            sim.step(&cmds);
        }
        sim.checksum()
    }

    /// Cross-arch determinism for the **ballistic + armour-facet** pipeline (invariant #7). The CI
    /// matrix runs `cargo test -p gonedark-core --release` on every target and any divergence is a
    /// desync — but `phase2`/`stress` are rifle squads (`muzzle_vel == 0`, unarmoured), so the
    /// shell + facet path they never touch would otherwise have NO cross-arch coverage. This pins a
    /// golden checksum after the full chain runs, so every arch must reproduce it bit-for-bit.
    #[test]
    fn infantry_seeds_player_and_five_dummies() {
        let mut sim = fresh();
        let inf = seed_infantry(&mut sim);
        let p = inf.player.index as usize;
        assert_eq!(sim.world.faction[p], Faction::Player);
        assert_eq!(sim.world.unit_kind[p], UnitKind::Rifleman);
        assert_eq!(sim.world.stance[p], Stance::FireAtWill);
        assert_eq!(sim.world.hull_heading[p], Angle(0)); // aims +X
        for e in [inf.open, inf.cover, inf.walled, inf.far, inf.flank] {
            let i = e.index as usize;
            assert_eq!(sim.world.faction[i], Faction::Enemy);
            // Dummies hold fire — the literal-executor AI never shoots back (invariant #3), so the
            // harness only sees the player's own shots.
            assert_eq!(sim.world.stance[i], Stance::HoldFire);
            assert_eq!(sim.world.health[i].max, INF_ENEMY_HP);
            assert_eq!(sim.world.unit_kind[i], UnitKind::Rifleman);
        }
    }

    #[test]
    fn infantry_cover_and_wall_are_placed() {
        let mut sim = fresh();
        let _ = seed_infantry(&mut sim);
        // Light cover sits on the cover dummy; the open dummy stands in the clear.
        assert_eq!(sim.terrain.cover_at(at(INF_COVER)), Cover::Light);
        assert_eq!(sim.terrain.cover_at(at(INF_OPEN)), Cover::None);
        // The Heavy wall blocks line of sight to `walled` but not to `open` (both within range).
        assert!(
            sim.terrain.line_of_sight(at((0, 0)), at(INF_OPEN)),
            "open is in the clear",
        );
        assert!(
            !sim.terrain.line_of_sight(at((0, 0)), at(INF_WALLED)),
            "the Heavy wall blocks the walled dummy",
        );
    }

    #[test]
    fn infantry_seeding_is_deterministic() {
        let mut a = fresh();
        let mut b = fresh();
        seed_infantry(&mut a);
        seed_infantry(&mut b);
        assert_eq!(a.checksum(), b.checksum());
    }

    #[test]
    fn ballistic_pipeline_is_deterministic() {
        let sum = run_ballistic_duel(130);
        // Stable on every arch (fixed-point only). Recompute + re-pin only on an *intended* change
        // to the duel scene/gun/armour or the ballistic/facet math; an *unexpected* change here is a
        // desync, not a value to bless.
        assert_eq!(sum, 0x287d_a2da_8990_2e31);
        // And it is reproducible run-to-run on this arch.
        assert_eq!(run_ballistic_duel(130), sum);
    }
}

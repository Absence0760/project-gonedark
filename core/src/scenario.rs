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

use crate::components::{
    Armor, BuildingKind, EntityKind, Faction, Health, Stance, UnitKind, Vec2, Weapon,
};
use crate::ecs::Entity;
use crate::economy;
use crate::fixed::Fixed;
use crate::sim::Sim;
use crate::terrain::Cover;
use crate::territory::ControlPoint;
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

// --- The skirmish: the first *real* (non-debug) match ------------------------------------------
//
// Unlike the duel / infantry sandboxes above — which isolate one mechanic for a harness — this is
// the playable two-base game: two operational bases, one starting troop each, and neutral "posts"
// to fight over. It is single-sourced in `core` for the same reason the debug scenes are: the
// desktop `app` and any headless driver seed the *identical* world, so the match you play is the
// match a harness could check, bit-for-bit (invariant #1/#2).
//
// Everything else the match needs already exists and is generic over the world this seeds: income
// + production ([`economy`](crate::economy)), capture ([`territory`](crate::territory)), the
// literal-executor units ([`orders`](crate::orders), invariant #3), and the scripted enemy
// [`commander`](crate::commander) the host drives. The host's win-condition evaluator decides the
// match (elimination / timeout); this seeder just sets the opening position.

/// Distance (world units) of each base from the centre line, on the X axis. The two bases sit at
/// `(∓SKIRMISH_BASE_X, 0)` — far enough apart that the no-man's-land in between is a real journey.
pub const SKIRMISH_BASE_X: i32 = 30;
/// How far *toward the centre* each base's starting troop spawns from its base, so the troop reads
/// as "stationed at the front of the base", not buried inside the camp footprint.
pub const SKIRMISH_TROOP_GAP: i32 = 4;
/// The two flank posts sit at `(0, ∓SKIRMISH_POST_FLANK_Y)`; the third is dead centre `(0, 0)`.
/// `14` keeps the posts more than a capture diameter apart (`2·CAPTURE_RADIUS = 12`), so a single
/// unit can't contest two at once.
pub const SKIRMISH_POST_FLANK_Y: i32 = 14;
/// The skirmish's deliberately small starting purse — one of the two **scenario-local** economy
/// levers (neither touches the locked D30 balance constants). With only this much banked, a faction
/// cannot mass an army turn-one. `100` = one Rifleman ([`economy::RIFLEMAN_COST`]): enough for a
/// single opening choice, no flood.
pub const SKIRMISH_START_PURSE: i64 = 100;

/// The skirmish's income **accrual period** (ticks between income accruals) — the second
/// scenario-local economy lever ([`Sim::set_income_period`](crate::sim::Sim::set_income_period)),
/// and the one that sets the *pace*. At the global 60 Hz, base income is `BASE_INCOME` (= 1) per
/// accrual, so accruing every `18` ticks gives `60/18 ≈ 3.3` gold/s → a Rifleman (`100`) roughly
/// **every 30 s** from base income alone, exactly the intended "slow by default" feel. It does NOT
/// touch the D30 constants — only the cadence stretches, so a held post still ~triples income
/// ([`economy::PER_POINT_INCOME`]): one post ⇒ ~10 s/Rifleman, all three ⇒ ~4 s. Capturing posts is
/// the whole "take a post to earn gold faster" loop, made literal.
pub const SKIRMISH_INCOME_PERIOD: u32 = 18;

/// The handles a seeded skirmish hands back: each side's operational base camp and its single
/// starting troop. The host embodies / selects `player_troop`; the enemy commander tasks
/// `enemy_troop` from its first plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Skirmish {
    /// The Player base camp at `(-SKIRMISH_BASE_X, 0)` — operational (produces immediately).
    pub player_base: Entity,
    /// The Enemy base camp at `(+SKIRMISH_BASE_X, 0)` — operational.
    pub enemy_base: Entity,
    /// The Player's single starting troop, just in front of the player base.
    pub player_troop: Entity,
    /// The Enemy's single starting troop, just in front of the enemy base.
    pub enemy_troop: Entity,
}

/// Seed `sim` with the two-base skirmish and return the [`Skirmish`] handles: two operational base
/// camps on opposite sides, **one starting troop each**, **three neutral posts** to capture, and the
/// small [`SKIRMISH_START_PURSE`]. The Enemy is left for the [`commander`](crate::commander) to drive
/// (no scripted opening order here), so the match plays out from this position.
///
/// Pure, deterministic, fixed-point (invariant #1): spawn order is fixed (posts, then both bases,
/// then both troops) and every value is integer / `Fixed`, so two seeds of a fresh `Sim` are
/// bit-identical — the property the single-sourced `app`/harness correspondence rests on.
pub fn seed_skirmish(sim: &mut Sim) -> Skirmish {
    // Slow the income drip to the skirmish's pace (scenario-local; the D30 constants are untouched).
    // Base income now reads as ~1 Rifleman / 30 s, and capturing posts is how you speed it up.
    sim.set_income_period(SKIRMISH_INCOME_PERIOD);

    // Three neutral posts strung across the no-man's-land: dead centre plus the two flanks. Holding
    // one ~triples a faction's income, so taking posts is how you out-produce the enemy.
    for post in [
        Vec2::new(Fixed::ZERO, Fixed::ZERO),
        Vec2::new(Fixed::ZERO, Fixed::from_int(SKIRMISH_POST_FLANK_Y)),
        Vec2::new(Fixed::ZERO, Fixed::from_int(-SKIRMISH_POST_FLANK_Y)),
    ] {
        sim.territory.points.push(ControlPoint::neutral(post));
    }

    // Pre-build both bases through the canonical `economy::build` path (so each camp's HP and
    // Building fields are exactly a produced camp's), funded from a temporary purse, then overwrite
    // the purse with the scenario's real, small starting value. Per-faction `Resources::new` gives
    // each side exactly one camp's worth, so both builds succeed.
    let base_x = Fixed::from_int(SKIRMISH_BASE_X);
    sim.resources = economy::Resources::new(economy::CAMP_BUILD_COST);
    let player_base = economy::build(
        &mut sim.world,
        &mut sim.resources,
        Faction::Player,
        BuildingKind::Camp,
        Vec2::new(-base_x, Fixed::ZERO),
    )
    .expect("the seed purse covers exactly one camp per faction");
    let enemy_base = economy::build(
        &mut sim.world,
        &mut sim.resources,
        Faction::Enemy,
        BuildingKind::Camp,
        Vec2::new(base_x, Fixed::ZERO),
    )
    .expect("the seed purse covers exactly one camp per faction");
    // Both bases start operational — this is a running match, not a fresh construction.
    sim.world.building[player_base.index as usize].build_ticks_left = 0;
    sim.world.building[enemy_base.index as usize].build_ticks_left = 0;
    // The real, deliberately small scenario purse (the scenario-local economy lever).
    sim.resources = economy::Resources::new(SKIRMISH_START_PURSE);

    // One starting troop per base. Full produced-Rifleman HP, `ReturnFire` stance (it fights back if
    // engaged but never auto-roams — invariant #3, it does exactly what it's ordered), facing the
    // enemy across the map. The player selects/commands theirs; the commander tasks the Enemy's.
    let troop_hp = economy::unit_stats(UnitKind::Rifleman).0.max;
    let troop_x = Fixed::from_int(SKIRMISH_BASE_X - SKIRMISH_TROOP_GAP);
    let player_troop = spawn_rifleman(
        sim,
        Vec2::new(-troop_x, Fixed::ZERO),
        Faction::Player,
        Stance::ReturnFire,
        troop_hp,
        Angle(0), // +X, toward the enemy
    );
    let enemy_troop = spawn_rifleman(
        sim,
        Vec2::new(troop_x, Fixed::ZERO),
        Faction::Enemy,
        Stance::ReturnFire,
        troop_hp,
        Angle(ANGLE_FULL / 2), // −X, toward the player
    );

    Skirmish {
        player_base,
        enemy_base,
        player_troop,
        enemy_troop,
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

    // --- the skirmish (the first real match) -----------------------------------------------

    /// Count alive `Unit`-kind entities of `faction` in `sim`.
    fn unit_count(sim: &Sim, faction: Faction) -> usize {
        (0..sim.world.capacity())
            .filter(|&i| {
                sim.world.is_index_alive(i)
                    && sim.world.kind[i] == EntityKind::Unit
                    && sim.world.faction[i] == faction
            })
            .count()
    }

    #[test]
    fn skirmish_seeds_two_operational_bases_one_troop_each() {
        let mut sim = fresh();
        let s = seed_skirmish(&mut sim);

        // Two operational base camps, one per faction, on opposite sides of the X axis.
        for (base, faction, sign) in [
            (s.player_base, Faction::Player, -1),
            (s.enemy_base, Faction::Enemy, 1),
        ] {
            let i = base.index as usize;
            assert_eq!(sim.world.kind[i], EntityKind::Building);
            assert_eq!(sim.world.faction[i], faction);
            assert_eq!(sim.world.building[i].kind, BuildingKind::Camp);
            assert_eq!(
                sim.world.building[i].build_ticks_left, 0,
                "a base starts operational (no construction)"
            );
            assert_eq!(sim.world.pos[i].x, Fixed::from_int(sign * SKIRMISH_BASE_X));
            assert!(sim.world.building[i].queue.is_empty(), "no pre-queued production");
        }

        // Exactly one starting troop per faction — a Rifleman on ReturnFire (invariant #3: it does
        // exactly what it's told, never auto-roams).
        for (troop, faction) in [
            (s.player_troop, Faction::Player),
            (s.enemy_troop, Faction::Enemy),
        ] {
            let i = troop.index as usize;
            assert_eq!(sim.world.kind[i], EntityKind::Unit);
            assert_eq!(sim.world.unit_kind[i], UnitKind::Rifleman);
            assert_eq!(sim.world.faction[i], faction);
            assert_eq!(sim.world.stance[i], Stance::ReturnFire);
        }
        // ...and *only* one each (no squads — the user's "each base starts with one troop").
        assert_eq!(unit_count(&sim, Faction::Player), 1);
        assert_eq!(unit_count(&sim, Faction::Enemy), 1);
    }

    #[test]
    fn skirmish_has_three_neutral_posts_no_one_holds_at_start() {
        let mut sim = fresh();
        seed_skirmish(&mut sim);
        assert_eq!(sim.territory.points.len(), 3, "three posts to fight over");
        assert!(
            sim.territory.points.iter().all(|p| p.owner == Faction::Neutral),
            "every post starts neutral"
        );
        // No starting troop sits on a post, so income opens at the base rate for both sides.
        assert_eq!(sim.territory.controlled_count(Faction::Player), 0);
        assert_eq!(sim.territory.controlled_count(Faction::Enemy), 0);
    }

    #[test]
    fn skirmish_starts_with_the_small_scenario_purse() {
        let mut sim = fresh();
        seed_skirmish(&mut sim);
        // The scenario-local economy levers: a small purse, identical for both combatants (the
        // build-cost dance is fully reset — neither base build leaves the purse skewed)...
        assert_eq!(sim.resources.get(Faction::Player), SKIRMISH_START_PURSE);
        assert_eq!(sim.resources.get(Faction::Enemy), SKIRMISH_START_PURSE);
        // ...and the slow income drip (≈1 Rifleman / 30 s from base income).
        assert_eq!(sim.income_period(), SKIRMISH_INCOME_PERIOD);
    }

    #[test]
    fn skirmish_seeding_is_deterministic() {
        // The single-sourcing property: two seeds of a fresh Sim are bit-identical (invariant #1),
        // so the played match and any headless driver agree.
        let mut a = fresh();
        let mut b = fresh();
        seed_skirmish(&mut a);
        seed_skirmish(&mut b);
        assert_eq!(a.checksum(), b.checksum());
    }

    /// End-to-end: the skirmish is a **live, evolving match**, not an inert tableau. Drive it the
    /// way the host does — the Enemy played by the scripted `commander` on its 1 s cadence, the
    /// Player sitting idle — and confirm the whole loop turns over on this scene: the enemy troop
    /// captures a post, income from it funds production, and reinforcements actually spawn. This is
    /// the integration the per-system unit tests can't give (each proves one wheel; this proves the
    /// gearbox), and it pins the scene against a future regression that would leave the match dead.
    ///
    /// Deterministic by construction: the commander reads only checksummed state + its own seeded
    /// RNG (no float, no `Sim::rng` draw), so this plays out identically on every run/arch.
    #[test]
    fn skirmish_plays_out_as_a_live_match_under_the_commander() {
        use crate::commander::{commander_orders, COMMANDER_PERIOD};
        use crate::rng::Rng;

        let mut sim = fresh();
        seed_skirmish(&mut sim);
        // The enemy commander's own stream (host seeds it `sim_seed ^ faction`); never `Sim::rng`.
        let mut enemy_rng = Rng::new(0xD0E1 ^ Faction::Enemy.index() as u64);

        // 30 s of play (60 Hz). The Player issues nothing — we isolate that the *enemy* side alone
        // makes the economy/capture/production loop turn.
        for _ in 0..(30 * crate::sim::TICK_HZ as u64) {
            let cmds = if sim.tick_count() % COMMANDER_PERIOD == 0 {
                commander_orders(
                    &sim.world,
                    &sim.territory,
                    &sim.resources,
                    &mut enemy_rng,
                    Faction::Enemy,
                    sim.tick_count(),
                )
            } else {
                Vec::new()
            };
            sim.step(&cmds);
        }

        // The commander sent its lone troop to take the nearest post → the Enemy now holds ground.
        assert!(
            sim.territory.controlled_count(Faction::Enemy) >= 1,
            "the enemy commander should have captured at least one post in 30 s"
        );
        // Post income + the base drip funded reinforcements → more than the one troop it started
        // with now fights for the Enemy (production actually spawned units on this scene).
        assert!(
            unit_count(&sim, Faction::Enemy) > 1,
            "the enemy should have produced reinforcements beyond its starting troop"
        );
        // The Player, untouched, still has exactly its starting troop (nothing auto-roamed it —
        // invariant #3: a unit with no order just holds).
        assert_eq!(
            unit_count(&sim, Faction::Player),
            1,
            "the idle player keeps exactly its one starting troop"
        );
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

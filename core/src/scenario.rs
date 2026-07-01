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
    Armor, Army, BuildingKind, EntityKind, Faction, Health, ShellKind, Stance, UnitKind, Vec2,
    Weapon,
};
use crate::ecs::Entity;
use crate::economy;
use crate::fixed::Fixed;
use crate::gunsmith::Loadout;
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
        reserve: 0,
        reserve_max: 0,
        turret_speed: 0,
        muzzle_vel: DUEL_GUN_MUZZLE_VEL,
        penetration: DUEL_GUN_PENETRATION,
        // Starts fully settled (P5): a stationary duel tank fires dead-on. The bloom only grows if
        // the embodied player drives/traverses, then settles back at rest.
        dispersion: Fixed::ZERO,
        // Loads AP by default (P6, D55) — solid-shot, the facet bounce/pen the duel demonstrates. A
        // harness/sandbox can `SelectShell` HE/APHE to exercise splash without touching the seeder.
        shell: ShellKind::Ap,
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
    // Pre-placed troops draw their loadout from their faction's per-Army roster (factions-plan
    // WS-B/WS-D): a US-side troop spawns the US logistics variant, a FR OPFOR troop the FR variant,
    // read through the matchup the scene already seeded with `set_army` (`sim.army_of`). A no-army
    // (legacy / debug) scene resolves [`Army::Neutral`], whose roster IS the shared `unit_stats`
    // baseline — so those scenes spawn the byte-identical pre-factions unit (WS-A discipline). The
    // tilt is logistics-only (mag/reload/reserve/turret); damage/cooldown/range/HP stay shared, so
    // the matchup stays fair (D71). Read before the mutable spawn so the immutable borrow is done.
    let (_default_hp, weapon) = economy::unit_stats_for(sim.army_of(faction), UnitKind::Rifleman);
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

/// Lay the skirmish's static cover map: a fair, four-fold-symmetric field of sandbag (`Light`) and
/// sight-blocking wall (`Heavy`) so each of the three posts is a real fight — forward cover to
/// advance between, corners that break line of sight, and a walled strongpoint bracketing each
/// flank post.
///
/// The layout is symmetric under **both** `x → −x` (the two bases mirror) and `y → −y` (the two
/// flank posts mirror), so neither side nor flank is favoured — the fairness invariant (#6) applied
/// to terrain. It is pure static map data (never mutated per tick, never in the checksum), and every
/// coordinate is an integer cell, so it is bit-identical on every build (invariant #1).
///
/// Movement is unaffected: the Phase-1 flow field carries no obstacle costs, so the walls shape
/// **fire and sight**, not pathing — a unit still walks the straight route and the terrain only
/// decides what it can shoot and be shot through on the way. The central base-to-base lane on
/// `y = 0` out past the posts is deliberately left free of `Heavy`, so a troop stationed at the
/// front of a base keeps a clear line straight down the middle.
fn build_skirmish_terrain(sim: &mut Sim) {
    let (cx, cy) = sim.terrain.cell_of(at((0, 0))); // world origin → centre cell (the pivot)

    // Place a rectangle at all four reflections of a centre-relative offset box, so one call lays a
    // fair, symmetric feature. `fill_rect` accepts corners in any order, so the reflected (negated)
    // spans need no re-sorting. Offsets are in cells (== world units), growing outward from the
    // centre; an axis-symmetric input simply overwrites the same cells four times (idempotent).
    let mut sym = |dx0: i32, dy0: i32, dx1: i32, dy1: i32, cover: Cover| {
        for &(sx, sy) in &[(1, 1), (-1, 1), (1, -1), (-1, -1)] {
            sim.terrain
                .fill_rect(cx + sx * dx0, cy + sy * dy0, cx + sx * dx1, cy + sy * dy1, cover);
        }
    };

    // 1. Forward sandbag line for each side: a vertical `Light` bar on the central lane between a
    //    base and the centre post — the covered position a push forms up on before crossing the
    //    open ground to the crossroads. (world x ≈ ±8, y ∈ [−2, 2].)
    sym(7, -2, 9, 2, Cover::Light);

    // 2. Centre-post nests: four `Light` sandbag blocks hugging the crossroads at its diagonals,
    //    with the axes left open — you fight *over* the post from cover, but the lanes onto it stay
    //    clear so it never becomes an impregnable bunker. (world (±2..3, ±2..3).)
    sym(2, 2, 3, 3, Cover::Light);

    // 3. Flank dividing wall with a central doorway: a `Heavy` wall between the centre and each
    //    flank post that breaks line of sight so a flank fight is its own space — pierced by a
    //    one-cell doorway on the central axis, a genuine chokepoint you funnel through to swing
    //    between the centre and a flank. (world y ≈ ±7, x ∈ [±1, ±5], open at x = 0.)
    sym(1, 6, 5, 8, Cover::Heavy);

    // 4. Flank strongpoints: a `Heavy` bunker bracketing each flank post on three sides (a back
    //    wall plus both flanks) and **open toward the centre**, so the post is a defensible
    //    objective you assault head-on from the contested middle, not a bare cell in the open. The
    //    flank post cell itself stays open (the bracket sits around it, at world y ∈ [±14, ±18]).
    sym(-3, 16, 3, 18, Cover::Heavy); // back wall, just beyond each flank post
    sym(3, 14, 3, 16, Cover::Heavy); // side walls closing the bracket toward the post
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

    // The faction matchup (factions-plan WS-A, D68): the Player fields the US Army, the Enemy the
    // French Army. Identity only — `Army` carries no per-tick checksum surface yet (the per-army
    // roster is WS-B), so seeding it leaves this scene byte-identical; it just records *which* armies
    // this match is between, ready for WS-B/WS-C to draw rosters and silhouettes from.
    sim.set_army(Faction::Player, Army::Us);
    sim.set_army(Faction::Enemy, Army::Fr);

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

    // One starting troop per base. Full produced-Rifleman HP, `FireAtWill` stance (the engagement
    // default — it shoots any enemy that comes into weapon range + LoS but still only *moves* on an
    // order, invariant #3: firing in place is not auto-roaming), facing the enemy across the map.
    // The player selects/commands theirs; the commander tasks the Enemy's. (ReturnFire would deadlock
    // the two starting troops — each would wait to be shot first, so they would just stare across the
    // map until the player embodied one and fired.)
    let troop_hp = economy::unit_stats(UnitKind::Rifleman).0.max;
    let troop_x = Fixed::from_int(SKIRMISH_BASE_X - SKIRMISH_TROOP_GAP);
    let player_troop = spawn_rifleman(
        sim,
        Vec2::new(-troop_x, Fixed::ZERO),
        Faction::Player,
        Stance::FireAtWill,
        troop_hp,
        Angle(0), // +X, toward the enemy
    );
    let enemy_troop = spawn_rifleman(
        sim,
        Vec2::new(troop_x, Fixed::ZERO),
        Faction::Enemy,
        Stance::FireAtWill,
        troop_hp,
        Angle(ANGLE_FULL / 2), // −X, toward the player
    );

    // Lay the static, fair cover map. It spawns nothing, so entity/spawn order — and thus the
    // per-tick checksum stream — is untouched; terrain is not in the checksum (invariant #7).
    build_skirmish_terrain(sim);

    Skirmish {
        player_base,
        enemy_base,
        player_troop,
        enemy_troop,
    }
}

// --- The *Seize* archetype: mission 1, "10 troops, take the base" (PvE WS-A) ---------------------
//
// The first PvE campaign mission (pve-campaign-plan WS-A). Unlike the skirmish — a two-base economy
// match — this is a **fixed-force assault**: ten player Riflemen and no base of their own, against
// one enemy camp (the base to take) defended by a small garrison. Production is OFF on both sides
// (no purse, a slow income drip), so the fight is decided by the ten troops you start with, not by
// out-producing the enemy. The *objective* layer that watches this scene live (capture-or-eliminate
// the base; fail = lose all ten) is **host-side** and never folds into the checksum — see
// `engine::objectives`. This seeder is the pure-`core` half: the same fixed-point, single-sourced
// world every surface seeds bit-identically (invariant #1/#2), exactly like `seed_skirmish`.

/// The player's ten troops spawn around `(SEIZE_PLAYER_X, 0)` on the west; the enemy base sits at
/// `(SEIZE_BASE_X, 0)` on the east — a real no-man's-land to cross under fire.
pub const SEIZE_PLAYER_X: i32 = -22;
/// X of the enemy base camp (the objective), on the east.
pub const SEIZE_BASE_X: i32 = 24;
/// How many troops the player commands in mission 1 ("10 troops"). A fixed force — there is no
/// production, so this is the whole army for the whole mission.
pub const SEIZE_TROOPS: usize = 10;

/// Garrison placements **relative to the enemy base**, in stable spawn order. A small defending
/// force around the camp the assault has to break through. Length is the garrison size.
const SEIZE_GARRISON_OFFSETS: [(i32, i32); 4] = [(-5, 4), (-5, -4), (-9, 0), (-1, 7)];

/// The handles a seeded *Seize* mission hands back: the player's ten troops, the enemy base camp
/// (capture-or-destroy to win), and its garrison. The host embodies/commands `troops`; the
/// objective layer watches the Enemy faction for elimination and the Player faction for a wipe.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SeizeMission {
    /// The player's ten Riflemen (no base — production is disabled), in stable spawn order.
    pub troops: Vec<Entity>,
    /// The enemy base camp — the objective. Operational (it would produce, but the empty purse
    /// disables that); destroying it (with the garrison) eliminates the Enemy and wins the mission.
    pub enemy_base: Entity,
    /// The garrison defending the base — FireAtWill Riflemen, in stable spawn order.
    pub garrison: Vec<Entity>,
}

impl SeizeMission {
    /// The enemy's total destroyable strength — the garrison plus the base camp. The objective
    /// layer uses this as the elimination progress goal ("N of M cleared") for the HUD.
    pub fn enemy_strength(&self) -> u32 {
        self.garrison.len() as u32 + 1
    }
}

/// Seed `sim` with the *Seize* mission ("10 troops, take the base") and return its [`SeizeMission`]
/// handles. Ten Player Riflemen on the west (no base — production is disabled), one operational
/// Enemy base camp on the east defended by a small FireAtWill garrison. Both purses are empty and
/// the income drip is throttled, so neither side reinforces: the mission is decided by the opening
/// ten troops. The player troops start `FireAtWill` (the engagement default — they shoot enemies in
/// range but only *move* on the host's order; invariant #3, firing in place is not auto-roaming);
/// the host commands them in.
///
/// Pure, deterministic, fixed-point (invariant #1): spawn order is fixed (troops, then the base,
/// then the garrison) and every value is integer / `Fixed`, so two seeds of a fresh `Sim` are
/// bit-identical — the single-sourcing property the played mission and any headless driver rest on.
///
/// This is the all-`Standard` loadout entry point ([`Loadout::STANDARD`] is a proven no-op on the
/// weapon, so the seeded world is byte-identical to the pre-gunsmith mission). To field the player's
/// chosen gunsmith loadout at match start, call [`seed_seize_mission_with_loadout`].
pub fn seed_seize_mission(sim: &mut Sim) -> SeizeMission {
    seed_seize_mission_with_loadout(sim, Loadout::STANDARD)
}

/// Seed the *Seize* mission, applying the player's chosen gunsmith [`Loadout`] to every one of the
/// ten Player troops' weapons **at match start** — the WS-C live-spawn wiring (D60, `customization.md`
/// §1). The loadout is **deterministic match-setup input**: it is applied once, here, on top of the
/// per-army base weapon (the Player fields [`Army::Us`], so the deltas are drawn from the US gunsmith
/// pool via [`Loadout::apply_to_weapon_for`]). The modified weapon fields are already hashed by
/// `Sim::fold`, so the loadout rides the per-tick checksum with **no new fold surface** — two peers
/// that pick the same loadout fold bit-identically, and a loadout desync would be caught by the
/// cross-arch matrix like any other sim divergence (invariant #7). The chosen build is a fair
/// sidegrade by construction (no strictly-dominant build — proven in [`crate::gunsmith`]).
///
/// [`seed_seize_mission`] is the `Loadout::STANDARD` (no-op) shim over this.
/// Lay the *Seize* mission's advance cover: staggered `Light` sandbag lines strung across the
/// no-man's-land the ten troops cross under the garrison's fire, so the assault has positions to
/// bound between instead of one naked charge (the "cross under fire" pillar made tactical).
///
/// Deliberately `Light`-only and **west of the garrison** (every bar at `x ≤ 8`; the garrison starts
/// at `x ≥ 15`): it shelters the attackers on the approach without fortifying the French defenders,
/// so the base stays exposed enough for the fixed-force assault to break it — the "won in time"
/// property the host-side objective rests on. No `Heavy` walls, so nothing blocks the assault's fire
/// onto the base. Pure static integer map data (invariant #1), never in the per-tick checksum.
fn build_seize_terrain(sim: &mut Sim) {
    // A vertical `Light` sandbag bar centred on world `(x, 0)`, `half` cells tall each way — already
    // symmetric across `y = 0`, so neither flank of the advance is the safer one. Three stations
    // stagger the approach from the deploy line toward the base's open killing ground.
    let mut bar = |x: i32, half: i32| {
        let (cx, cy) = sim.terrain.cell_of(at((x, 0)));
        sim.terrain.fill_rect(cx, cy - half, cx, cy + half, Cover::Light);
    };
    bar(-12, 3); // first bound out of the deploy line
    bar(-2, 4); //  the midfield sandbag wall
    bar(8, 3); //   the last cover before the base's open ground
}

pub fn seed_seize_mission_with_loadout(sim: &mut Sim, player_loadout: Loadout) -> SeizeMission {
    // Production OFF: no purse for either side and a slow income drip, so this stays a fixed-force
    // assault rather than an economy race. The player has no camp at all (so it cannot produce); the
    // enemy camp is the objective and, with an empty purse, its commander cannot reinforce.
    sim.set_income_period(600);

    // The PvE matchup (factions-plan WS-A/WS-D, D68): the campaign is played US-side, with the French
    // Army as the OPFOR — so factions debut in PvE. Identity only (no per-army stats until WS-B), so
    // this is byte-neutral; it records the matchup for WS-B/WS-C to render distinctly.
    sim.set_army(Faction::Player, Army::Us);
    sim.set_army(Faction::Enemy, Army::Fr);

    // Ten Player Riflemen in a 2x5 block on the west, full produced HP, FireAtWill (the engagement
    // default — they shoot any enemy that comes into range as they assault, but only *move* on the
    // host's order; invariant #3), facing the base.
    let troop_hp = economy::unit_stats(UnitKind::Rifleman).0.max;
    let mut troops = Vec::with_capacity(SEIZE_TROOPS);
    for col in 0..5 {
        for &row_y in &[-2, 2] {
            let x = SEIZE_PLAYER_X - col * 2;
            troops.push(spawn_rifleman(
                sim,
                at((x, row_y)),
                Faction::Player,
                Stance::FireAtWill,
                troop_hp,
                Angle(0), // +X, toward the base
            ));
        }
    }

    // Apply the player's chosen gunsmith loadout to every assault troop's weapon — the WS-C
    // live-spawn step. Match-setup input applied once on top of the per-army base weapon (drawn from
    // the Player's army gunsmith pool); `Loadout::STANDARD` is a no-op, so an opted-out player's
    // troops keep the byte-identical baseline weapon. The modified fields are all already in
    // `Sim::fold`, so this folds into the per-tick checksum with no new fold surface (invariant #7).
    let player_army = sim.army_of(Faction::Player);
    for &t in &troops {
        player_loadout.apply_to_weapon_for(player_army, &mut sim.world.weapon[t.index as usize]);
    }

    // The enemy base camp (the objective). Built through the canonical `economy::build` path from a
    // temporary one-camp purse so its HP/Building fields match a produced camp, then made operational
    // and the purse reset to empty (production disabled).
    sim.resources = economy::Resources::new(economy::CAMP_BUILD_COST);
    let enemy_base = economy::build(
        &mut sim.world,
        &mut sim.resources,
        Faction::Enemy,
        BuildingKind::Camp,
        at((SEIZE_BASE_X, 0)),
    )
    .expect("the temporary seed purse covers exactly one camp");
    sim.world.building[enemy_base.index as usize].build_ticks_left = 0;
    // Empty both purses: no production for either side (the fixed-force assault).
    sim.resources = economy::Resources::new(0);

    // A small garrison defending the base — FireAtWill Riflemen (they engage the assault on sight),
    // facing the incoming player line.
    let mut garrison = Vec::with_capacity(SEIZE_GARRISON_OFFSETS.len());
    for &(dx, dy) in &SEIZE_GARRISON_OFFSETS {
        garrison.push(spawn_rifleman(
            sim,
            at((SEIZE_BASE_X + dx, dy)),
            Faction::Enemy,
            Stance::FireAtWill,
            troop_hp,
            Angle(ANGLE_FULL / 2), // −X, toward the player line
        ));
    }

    // Lay the static advance cover across the no-man's-land. It spawns nothing (entity/spawn order
    // and the checksum stream are untouched) and shelters only the approach, not the defenders.
    build_seize_terrain(sim);

    SeizeMission {
        troops,
        enemy_base,
        garrison,
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
    fn skirmish_embodied_fire_kills_the_unit_in_front_not_the_base_behind() {
        use crate::components::{InputSource, Stance};
        let mut sim = fresh();
        let s = seed_skirmish(&mut sim);
        let p = s.player_troop.index as usize;
        let e = s.enemy_troop.index as usize;
        sim.world.input_source[p] = InputSource::Embodied;
        sim.world.stance[e] = Stance::HoldFire; // isolate: only the player's shots matter
        // Stand the player 5 units west of the enemy troop; the enemy base sits further east on the
        // same +X line. Regression: the shot must kill the troop in front, not be soaked by the
        // lower-index enemy base behind it (the original "impossible to kill an enemy" report).
        sim.world.pos[p] = Vec2::new(sim.world.pos[e].x - Fixed::from_int(5), sim.world.pos[e].y);
        for _ in 0..300 {
            sim.step(&[Command::Fire { entity: s.player_troop, dir: plus_x() }]);
            if !sim.world.is_alive(s.enemy_troop) {
                break;
            }
        }
        assert!(
            !sim.world.is_alive(s.enemy_troop),
            "the embodied player kills the troop it is aiming at",
        );
        assert!(sim.world.is_alive(s.enemy_base), "the base behind it is not what got shot");
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

        // Exactly one starting troop per faction — a Rifleman on FireAtWill (the engagement default:
        // it shoots enemies in range, but only moves on an order — invariant #3, firing ≠ roaming).
        for (troop, faction) in [
            (s.player_troop, Faction::Player),
            (s.enemy_troop, Faction::Enemy),
        ] {
            let i = troop.index as usize;
            assert_eq!(sim.world.kind[i], EntityKind::Unit);
            assert_eq!(sim.world.unit_kind[i], UnitKind::Rifleman);
            assert_eq!(sim.world.faction[i], faction);
            assert_eq!(sim.world.stance[i], Stance::FireAtWill);
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

    // --- the skirmish cover map (fair, symmetric, tactical) --------------------------------------

    #[test]
    fn skirmish_terrain_is_four_fold_symmetric_and_fair() {
        // Fairness (#6) at the terrain level: the cover map is identical under both x→−x (the two
        // bases mirror) and y→−y (the two flank posts mirror), so no side or flank is favoured.
        use crate::flow_field::GRID;
        let mut sim = fresh();
        seed_skirmish(&mut sim);
        let (cx, cy) = sim.terrain.cell_of(at((0, 0)));
        let g = GRID as i32;
        // At least one Heavy and one Light cell exist, so the symmetry check is over a real map.
        let mut saw_heavy = false;
        let mut saw_light = false;
        for y in 0..g {
            for x in 0..g {
                let here = sim.terrain.cover_at_cell(x, y);
                saw_heavy |= here == Cover::Heavy;
                saw_light |= here == Cover::Light;
                // Reflect across the centre on each axis; every mirror must carry identical cover.
                let (mx, my) = (2 * cx - x, 2 * cy - y);
                assert_eq!(here, sim.terrain.cover_at_cell(mx, y), "x-mirror at ({x},{y})");
                assert_eq!(here, sim.terrain.cover_at_cell(x, my), "y-mirror at ({x},{y})");
                assert_eq!(here, sim.terrain.cover_at_cell(mx, my), "xy-mirror at ({x},{y})");
            }
        }
        assert!(saw_heavy, "the map has sight-blocking Heavy walls");
        assert!(saw_light, "the map has Light sandbag cover");
    }

    #[test]
    fn skirmish_terrain_has_a_flank_doorway_and_sight_walls() {
        let mut sim = fresh();
        seed_skirmish(&mut sim);
        let center = at((0, 0));
        let north_flank = at((0, SKIRMISH_POST_FLANK_Y));
        // The dividing wall breaks a sightline OFF the central axis, from the centre lane up to the
        // flank post — you cannot freely shoot the flank fight from the middle.
        assert!(
            !sim.terrain.line_of_sight(at((3, 0)), north_flank),
            "the flank dividing wall breaks sight from the centre lane to the flank post",
        );
        // ...but the one-cell doorway on the central axis is open: dead-centre to the flank post has
        // LoS — the chokepoint you must funnel through to swing between centre and flank.
        assert!(
            sim.terrain.line_of_sight(center, north_flank),
            "the central doorway is an open chokepoint between the centre and the flank",
        );
        // Symmetric to the south flank by construction.
        assert!(sim.terrain.line_of_sight(center, at((0, -SKIRMISH_POST_FLANK_Y))));
    }

    #[test]
    fn skirmish_terrain_keeps_the_base_lane_open_and_gives_forward_cover() {
        let mut sim = fresh();
        let s = seed_skirmish(&mut sim);
        // Forward Light cover sits on the central lane for each side to advance from — and it is the
        // sight-passing kind, not a wall.
        assert_eq!(sim.terrain.cover_at(at((8, 0))), Cover::Light, "player-side forward cover");
        assert_eq!(sim.terrain.cover_at(at((-8, 0))), Cover::Light, "enemy-side forward cover");
        // The regression the embodied-fire test depends on: the central y = 0 lane carries NO
        // sight-blocking wall, so the two front-line troops can see straight down the middle.
        let p = sim.world.pos[s.player_troop.index as usize];
        let e = sim.world.pos[s.enemy_troop.index as usize];
        assert!(
            sim.terrain.line_of_sight(p, e),
            "the two front-line troops see each other down the open central lane",
        );
        // The three posts themselves stay open ground (you fight over them from cover, not from
        // inside a wall): none of the post cells is Heavy.
        for post in [at((0, 0)), at((0, SKIRMISH_POST_FLANK_Y)), at((0, -SKIRMISH_POST_FLANK_Y))] {
            assert_ne!(sim.terrain.cover_at(post), Cover::Heavy, "a post is never a walled cell");
        }
    }

    #[test]
    fn skirmish_seeding_is_deterministic_including_terrain() {
        // The cover map is pure integer static data, so two seeds produce byte-identical terrain
        // across the whole grid (invariant #1) — the single-sourcing property extended to the map.
        use crate::flow_field::GRID;
        let mut a = fresh();
        let mut b = fresh();
        seed_skirmish(&mut a);
        seed_skirmish(&mut b);
        for cy in 0..GRID as i32 {
            for cx in 0..GRID as i32 {
                assert_eq!(a.terrain.cover_at_cell(cx, cy), b.terrain.cover_at_cell(cx, cy));
            }
        }
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
        use crate::commander::{commander_orders, CommanderConfig, COMMANDER_PERIOD};
        use crate::rng::Rng;

        let mut sim = fresh();
        let s = seed_skirmish(&mut sim);
        // The enemy commander's own stream (host seeds it `sim_seed ^ faction`); never `Sim::rng`.
        let mut enemy_rng = Rng::new(0xD0E1 ^ Faction::Enemy.index() as u64);

        // The player troop is issued NO order, so the literal executor (invariant #3) must never move
        // it under its own power — track its spawn position and prove it stays put for as long as it
        // lives. (It IS on FireAtWill, the engagement default, so it shoots back if reached — and may
        // be overrun by the enemy army it built — but a stance is a *firing* posture, not a licence to
        // roam: firing in place is not movement.)
        let pi = s.player_troop.index as usize;
        let player_spawn = sim.world.pos[pi];

        // 30 s of play (60 Hz). The Player issues nothing — we isolate that the *enemy* side alone
        // makes the economy/capture/production loop turn.
        for _ in 0..(30 * crate::sim::TICK_HZ as u64) {
            let cmds = if sim.tick_count().is_multiple_of(COMMANDER_PERIOD) {
                commander_orders(
                    &sim.world,
                    &sim.territory,
                    &sim.resources,
                    &mut enemy_rng,
                    &CommanderConfig::default(),
                    &[],
                    Faction::Enemy,
                    sim.tick_count(),
                )
            } else {
                Vec::new()
            };
            sim.step(&cmds);
            // No-auto-roam check, every tick it is alive: an unordered unit holds its ground.
            if sim.world.is_alive(s.player_troop) {
                assert_eq!(
                    sim.world.pos[pi], player_spawn,
                    "the unordered player troop must never move on its own (invariant #3)",
                );
            }
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
    }

    #[test]
    fn ballistic_pipeline_is_deterministic() {
        let sum = run_ballistic_duel(130);
        // Stable on every arch (fixed-point only). Recompute + re-pin only on an *intended* change
        // to the duel scene/gun/armour or the ballistic/facet math; an *unexpected* change here is a
        // desync, not a value to bless. (D67: re-pinned after the Weapon fold grew reserve +
        // reserve_max — every slot now folds two more u32, so the stream shifted by design.
        // D55 P5+P6: re-pinned after the Weapon fold grew a `dispersion` word + a loaded-shell tag and
        // the projectile fold grew a shell tag + splash pair per slot. The duel tank fires from a
        // standstill (dispersion stays 0, AP is the default shell), so the shells fly identically;
        // only the raw stream value shifted by the appended fields, by design.)
        assert_eq!(sum, 0xad57_73c4_5e4d_08d7);
        // And it is reproducible run-to-run on this arch.
        assert_eq!(run_ballistic_duel(130), sum);
    }

    // --- the *Seize* mission (PvE WS-A, mission 1) -----------------------------------------------

    #[test]
    fn seize_seeds_ten_player_troops_no_player_base() {
        let mut sim = fresh();
        let m = seed_seize_mission(&mut sim);
        assert_eq!(m.troops.len(), SEIZE_TROOPS, "the player commands exactly ten troops");
        // Exactly ten Player units, and NO Player building (production is disabled — no camp).
        assert_eq!(unit_count(&sim, Faction::Player), SEIZE_TROOPS);
        for &t in &m.troops {
            let i = t.index as usize;
            assert_eq!(sim.world.kind[i], EntityKind::Unit);
            assert_eq!(sim.world.unit_kind[i], UnitKind::Rifleman);
            assert_eq!(sim.world.faction[i], Faction::Player);
            assert_eq!(sim.world.stance[i], Stance::FireAtWill);
        }
        let player_buildings = (0..sim.world.capacity()).filter(|&i| {
            sim.world.is_index_alive(i)
                && sim.world.kind[i] == EntityKind::Building
                && sim.world.faction[i] == Faction::Player
        });
        assert_eq!(player_buildings.count(), 0, "the player has no base — it cannot produce");
    }

    #[test]
    fn seize_seeds_an_operational_enemy_base_and_garrison() {
        let mut sim = fresh();
        let m = seed_seize_mission(&mut sim);
        let b = m.enemy_base.index as usize;
        assert_eq!(sim.world.kind[b], EntityKind::Building);
        assert_eq!(sim.world.faction[b], Faction::Enemy);
        assert_eq!(sim.world.building[b].kind, BuildingKind::Camp);
        assert_eq!(sim.world.building[b].build_ticks_left, 0, "the base starts operational");
        assert!(sim.world.building[b].queue.is_empty(), "no pre-queued production");
        assert_eq!(sim.world.pos[b].x, Fixed::from_int(SEIZE_BASE_X));
        // The garrison defends FireAtWill; the base + garrison is the enemy strength the objective
        // tracks for the HUD progress bar.
        assert!(!m.garrison.is_empty(), "the base has a defending garrison");
        for &g in &m.garrison {
            let i = g.index as usize;
            assert_eq!(sim.world.faction[i], Faction::Enemy);
            assert_eq!(sim.world.unit_kind[i], UnitKind::Rifleman);
            assert_eq!(sim.world.stance[i], Stance::FireAtWill);
        }
        assert_eq!(m.enemy_strength(), m.garrison.len() as u32 + 1, "garrison + the base camp");
    }

    #[test]
    fn seize_terrain_covers_the_advance_but_not_the_garrison() {
        let mut sim = fresh();
        let m = seed_seize_mission(&mut sim);
        // The no-man's-land carries Light advance cover the assault bounds between (the midfield
        // sandbag wall), and it is the sight-passing kind — never a Heavy wall that would stall the
        // assault's fire onto the base.
        assert_eq!(sim.terrain.cover_at(at((-2, 0))), Cover::Light, "midfield sandbag cover exists");
        // The garrison and base stay in the open: the terrain does NOT fortify the defenders, so the
        // fixed-force assault can still break them (the won-in-time property the objective rests on).
        for &g in &m.garrison {
            assert_eq!(
                sim.terrain.cover_at(sim.world.pos[g.index as usize]),
                Cover::None,
                "the garrison defends in the open, not from terrain cover",
            );
        }
        assert_eq!(
            sim.terrain.cover_at(sim.world.pos[m.enemy_base.index as usize]),
            Cover::None,
            "the objective base is not shielded by cover",
        );
    }

    #[test]
    fn seize_disables_production_empty_purses() {
        let mut sim = fresh();
        let _ = seed_seize_mission(&mut sim);
        // No purse for either side — neither can produce, so the mission is a fixed-force assault.
        assert_eq!(sim.resources.get(Faction::Player), 0);
        assert_eq!(sim.resources.get(Faction::Enemy), 0);
    }

    #[test]
    fn seize_seeding_is_deterministic() {
        // Single-sourcing: two seeds of a fresh Sim are bit-identical (invariant #1), so the played
        // mission and any headless objective-driver agree.
        let mut a = fresh();
        let mut b = fresh();
        seed_seize_mission(&mut a);
        seed_seize_mission(&mut b);
        assert_eq!(a.checksum(), b.checksum());
    }

    // --- factions WS-A: the seeded matchup -----------------------------------------------------

    #[test]
    fn skirmish_seeds_the_us_vs_french_matchup() {
        // factions-plan WS-A/D68: the real match fields the US Army (Player) vs the French Army
        // (Enemy). Neutral (uncontrolled posts) stays non-aligned.
        let mut sim = fresh();
        seed_skirmish(&mut sim);
        assert_eq!(sim.army_of(Faction::Player), Army::Us);
        assert_eq!(sim.army_of(Faction::Enemy), Army::Fr);
        assert_eq!(sim.army_of(Faction::Neutral), Army::Neutral);
    }

    #[test]
    fn seize_seeds_the_us_vs_french_matchup() {
        // The PvE debut (WS-A/WS-D): played US-side against a French OPFOR.
        let mut sim = fresh();
        seed_seize_mission(&mut sim);
        assert_eq!(sim.army_of(Faction::Player), Army::Us);
        assert_eq!(sim.army_of(Faction::Enemy), Army::Fr);
    }

    #[test]
    fn debug_scenes_field_no_army_so_they_stay_byte_unchanged() {
        // The duel/infantry debug scenes select NO army — every faction stays Army::Neutral. Because
        // the selection is not folded into the per-tick checksum, a no-army scene is byte-identical to
        // before factions existed (their golden checksums are unmoved — see `ballistic_pipeline_*` and
        // the sim-runner duel/infantry goldens). This pins that the seeders leave them non-aligned.
        let mut duel_sim = fresh();
        seed_duel(&mut duel_sim);
        let mut inf_sim = fresh();
        seed_infantry(&mut inf_sim);
        for sim in [&duel_sim, &inf_sim] {
            for f in Faction::ALL {
                assert_eq!(sim.army_of(f), Army::Neutral, "a debug scene fields no real army");
            }
        }
    }

    // --- factions WS-D: army-tilted pre-placed starting troops --------------------------------
    //
    // WS-A/WS-D seed the US-vs-FR *matchup* (above); these pin that the pre-placed starting troops
    // are actually *composed from* each side's per-Army roster (WS-B's `unit_stats_for`): a US-side
    // opening force spawns the US logistics variant, a FR OPFOR force the FR variant — not the bare
    // shared baseline. A no-army (legacy) scene keeps the byte-identical baseline loadout.

    #[test]
    fn skirmish_pre_places_army_correct_starting_troops() {
        // The US player troop and FR enemy troop each spawn their OWN army's roster variant — the
        // exact `unit_stats_for(army, Rifleman)` loadout the produced units would field, so seeded
        // and produced troops match on each side.
        let mut sim = fresh();
        let s = seed_skirmish(&mut sim);

        let us_rifle = economy::unit_stats_for(Army::Us, UnitKind::Rifleman).1;
        let fr_rifle = economy::unit_stats_for(Army::Fr, UnitKind::Rifleman).1;
        assert_eq!(sim.world.weapon[s.player_troop.index as usize], us_rifle, "US player troop fields the US variant");
        assert_eq!(sim.world.weapon[s.enemy_troop.index as usize], fr_rifle, "FR OPFOR troop fields the FR variant");
        // The two armies' opening troops are *distinct* (the logistics tilt makes them read apart),
        // and neither is the shared baseline.
        let baseline = economy::unit_stats(UnitKind::Rifleman).1;
        assert_ne!(us_rifle, fr_rifle, "the two armies' troops differ");
        assert_ne!(us_rifle, baseline, "the US troop is tilted off the baseline");
        assert_ne!(fr_rifle, baseline, "the FR troop is tilted off the baseline");
    }

    #[test]
    fn seize_pre_places_an_army_correct_opening_force() {
        // The PvE *Seize* mission: every one of the ten player troops is US-rostered, every garrison
        // defender is FR-rostered — a US assault force vs a French OPFOR garrison, each statted from
        // its own army (WS-B/WS-D).
        let mut sim = fresh();
        let m = seed_seize_mission(&mut sim);

        let us_rifle = economy::unit_stats_for(Army::Us, UnitKind::Rifleman).1;
        let fr_rifle = economy::unit_stats_for(Army::Fr, UnitKind::Rifleman).1;
        for &t in &m.troops {
            assert_eq!(sim.world.weapon[t.index as usize], us_rifle, "every US assault troop fields the US variant");
        }
        for &g in &m.garrison {
            assert_eq!(sim.world.weapon[g.index as usize], fr_rifle, "every FR garrison defender fields the FR variant");
        }
        assert_ne!(us_rifle, fr_rifle, "the assault and the garrison read as different armies");
    }

    #[test]
    fn legacy_infantry_troops_keep_the_byte_identical_baseline_loadout() {
        // The WS-A discipline at the loadout level: a no-army scene (the infantry sandbox) spawns the
        // EXACT shared `unit_stats` baseline weapon for every troop — byte-for-byte the pre-factions
        // unit. (This is what keeps the duel/infantry sim-runner goldens unmoved.)
        let mut sim = fresh();
        let inf = seed_infantry(&mut sim);
        let baseline = economy::unit_stats(UnitKind::Rifleman).1;
        for e in [inf.player, inf.open, inf.cover, inf.walled, inf.far, inf.flank] {
            assert_eq!(sim.world.weapon[e.index as usize], baseline, "a no-army troop keeps the baseline loadout");
        }
    }

    // --- WS-C: gunsmith loadout applied at live match start --------------------------------------
    //
    // The live-spawn wiring (D60): `seed_seize_mission_with_loadout` applies the player's chosen
    // gunsmith `Loadout` to every assault troop's weapon at match start as deterministic match-setup
    // input. These pin (a) it lands on the live-spawned weapon, (b) the per-tick checksum agrees for
    // same-loadout peers and diverges *only as expected sim state* for different ones (invariant #7),
    // and (c) the no-strictly-dominant-build fairness rule (D30/D60) holds on the live-spawn path —
    // not just on the `core::gunsmith` delta model.

    /// Every loadout in the full build space (3 slots × 3 options = 27).
    fn all_loadouts() -> Vec<Loadout> {
        use crate::gunsmith::{Barrel, Magazine, Optic};
        let mut v = Vec::new();
        for &optic in &Optic::ALL {
            for &barrel in &Barrel::ALL {
                for &magazine in &Magazine::ALL {
                    v.push(Loadout { optic, barrel, magazine });
                }
            }
        }
        v
    }

    /// The player troop[0]'s weapon after the *Seize* mission is live-seeded with `loadout`.
    fn seize_player_weapon(loadout: Loadout) -> Weapon {
        let mut sim = fresh();
        let m = seed_seize_mission_with_loadout(&mut sim, loadout);
        sim.world.weapon[m.troops[0].index as usize]
    }

    /// Polarity-aware "no axis worse" over the six tracked weapon stat axes (the same polarity as
    /// `StatDelta`: range/damage/mag_size/reserve better-when-higher; cooldown/reload better-when-
    /// lower). Used to assert no live-spawned build strictly dominates another.
    fn weapon_strictly_dominates(a: &Weapon, b: &Weapon) -> bool {
        let no_axis_worse = a.range >= b.range
            && a.damage >= b.damage
            && a.cooldown_ticks <= b.cooldown_ticks
            && a.mag_size >= b.mag_size
            && a.reload_ticks <= b.reload_ticks
            && a.reserve >= b.reserve;
        let some_axis_better = a.range > b.range
            || a.damage > b.damage
            || a.cooldown_ticks < b.cooldown_ticks
            || a.mag_size > b.mag_size
            || a.reload_ticks < b.reload_ticks
            || a.reserve > b.reserve;
        no_axis_worse && some_axis_better
    }

    /// Drive the *Seize* mission seeded with `loadout` through the scripted assault and return the
    /// per-tick checksum stream (pre-step first, then one entry per stepped tick). Deterministic by
    /// construction — the lockstep building block the agreement/divergence tests below replay. The
    /// loadout is in the weapon component, so it is folded from the pre-step checksum onward.
    fn seize_checksum_stream(loadout: Loadout, ticks: u64) -> Vec<u64> {
        use crate::commander::{commander_orders, CommanderConfig, COMMANDER_PERIOD};
        use crate::rng::Rng;

        let mut sim = fresh();
        let m = seed_seize_mission_with_loadout(&mut sim, loadout);
        let base_pos = sim.world.pos[m.enemy_base.index as usize];
        let mut enemy_rng = Rng::new(0xD0E1 ^ Faction::Enemy.index() as u64);

        let mut stream = Vec::with_capacity(ticks as usize + 1);
        // Pre-step: the loadout already lives in the weapon fold, so peers must agree (or diverge)
        // here before a single system runs.
        stream.push(sim.checksum());

        let opening: Vec<Command> = m
            .troops
            .iter()
            .map(|&t| Command::AttackMove { entity: t, target: base_pos })
            .collect();
        sim.step(&opening);
        stream.push(sim.checksum());

        for _ in 1..ticks {
            let cmds = if sim.tick_count().is_multiple_of(COMMANDER_PERIOD) {
                commander_orders(
                    &sim.world,
                    &sim.territory,
                    &sim.resources,
                    &mut enemy_rng,
                    &CommanderConfig::default(),
                    &[],
                    Faction::Enemy,
                    sim.tick_count(),
                )
            } else {
                Vec::new()
            };
            sim.step(&cmds);
            stream.push(sim.checksum());
        }
        stream
    }

    /// The live-spawn application: every assault troop's weapon is the player-army (US) base Rifleman
    /// with the chosen loadout applied — exactly what `Loadout::apply_to_weapon_for(Army::Us, …)`
    /// produces — and the enemy garrison is untouched by the *player's* gunsmith.
    #[test]
    fn seize_applies_the_chosen_loadout_to_every_player_troop() {
        use crate::gunsmith::{Barrel, Magazine, Optic};
        let loadout = Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
        };
        let mut sim = fresh();
        let m = seed_seize_mission_with_loadout(&mut sim, loadout);

        let mut expected = economy::unit_stats_for(Army::Us, UnitKind::Rifleman).1;
        loadout.apply_to_weapon_for(Army::Us, &mut expected);
        for &t in &m.troops {
            assert_eq!(
                sim.world.weapon[t.index as usize], expected,
                "each US assault troop fields the US-pool weapon with the loadout applied"
            );
        }
        // A non-Standard loadout actually moved the weapon off the bare US baseline.
        let bare_us = economy::unit_stats_for(Army::Us, UnitKind::Rifleman).1;
        assert_ne!(expected, bare_us, "the loadout is a real change off the baseline");
        // The enemy (FR) garrison is unaffected — this is the player's gunsmith, not the enemy's.
        let fr_rifle = economy::unit_stats_for(Army::Fr, UnitKind::Rifleman).1;
        for &g in &m.garrison {
            assert_eq!(
                sim.world.weapon[g.index as usize], fr_rifle,
                "the enemy garrison is untouched by the player loadout"
            );
        }
    }

    /// `Loadout::STANDARD` is a true no-op on the live-spawn path: the mission seeded with the
    /// Standard loadout is byte-identical to the plain `seed_seize_mission` (same handles, same
    /// checksum) — so an opted-out player's match is unchanged and existing goldens are unmoved.
    #[test]
    fn seize_standard_loadout_is_byte_identical_to_the_plain_seeder() {
        let mut plain = fresh();
        let mut std = fresh();
        let m_plain = seed_seize_mission(&mut plain);
        let m_std = seed_seize_mission_with_loadout(&mut std, Loadout::STANDARD);
        assert_eq!(m_plain, m_std, "the Standard loadout seeds the identical handles");
        assert_eq!(
            plain.checksum(),
            std.checksum(),
            "the Standard loadout leaves the seeded world byte-identical"
        );
    }

    /// **Checksum agreement (invariant #7).** Two peers seeding the SAME loadout and replaying the
    /// SAME scripted assault produce the identical per-tick checksum stream — the loadout rides the
    /// existing weapon fold, adding no desync surface.
    #[test]
    fn same_loadout_seize_two_peers_agree_every_tick() {
        use crate::gunsmith::{Barrel, Magazine, Optic};
        let loadout = Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Quickdraw,
        };
        let a = seize_checksum_stream(loadout, 400);
        let b = seize_checksum_stream(loadout, 400);
        assert_eq!(
            a, b,
            "two peers with the same loadout produce the identical per-tick checksum stream"
        );
    }

    /// **Honest divergence.** Two peers seeding DIFFERENT loadouts (same scene, same scripted input)
    /// diverge — and the divergence is *expected sim state*: it shows up from the pre-step checksum
    /// (the weapon component differs) and persists through the fight. The same-loadout control above
    /// proves the divergence is caused by the loadout, not by nondeterminism — so a real loadout
    /// desync would surface on the cross-arch matrix, never drift silently.
    #[test]
    fn different_loadout_seize_diverges_only_as_expected_sim_state() {
        use crate::gunsmith::{Barrel, Magazine, Optic};
        let marksman = Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
        };
        let runner = Loadout {
            optic: Optic::CloseQuarters,
            barrel: Barrel::Light,
            magazine: Magazine::Quickdraw,
        };
        let a = seize_checksum_stream(marksman, 400);
        let b = seize_checksum_stream(runner, 400);
        assert_ne!(
            a[0], b[0],
            "different loadouts fold to a different pre-step checksum (the weapon stats differ)"
        );
        assert_ne!(a, b, "different loadouts produce a different sim trajectory");
    }

    /// **Fairness on the live-spawn path (D30/D60).** Extends the no-strictly-dominant-build property
    /// from the `core::gunsmith` delta model to the actual *seeded* weapons: across all 27 loadouts,
    /// the live-spawned player Rifleman of one build never strictly dominates another on the six
    /// tracked axes. A future re-tune (of the table OR the base weapon) that made one build a flat
    /// upgrade in-match trips this.
    #[test]
    fn no_seize_loadout_strictly_dominates_another_on_the_live_weapon() {
        let weapons: Vec<Weapon> = all_loadouts().iter().map(|&l| seize_player_weapon(l)).collect();
        for (i, a) in weapons.iter().enumerate() {
            for (j, b) in weapons.iter().enumerate() {
                if i == j {
                    continue;
                }
                assert!(
                    !weapon_strictly_dominates(a, b),
                    "live-spawned build {i} strictly dominates build {j} — not a sidegrade",
                );
            }
        }
    }

    /// Drive the US-vs-FR *Seize* mission to a result and return `(final_checksum, enemy_cleared)`:
    /// the player's ten US troops attack-move onto the French base while the enemy commander defends,
    /// run until the garrison is broken or `max_ticks` elapses. Deterministic by construction (the
    /// commander reads only checksummed state + its own seeded stream; every value is fixed-point), so
    /// this is the lockstep building block the agreement test below replays.
    fn drive_us_vs_fr_seize(max_ticks: u64) -> (u64, u32) {
        use crate::commander::{commander_orders, CommanderConfig, COMMANDER_PERIOD};
        use crate::components::Stance;
        use crate::rng::Rng;

        let mut sim = fresh();
        let m = seed_seize_mission(&mut sim);
        let base_pos = sim.world.pos[m.enemy_base.index as usize];
        let mut enemy_rng = Rng::new(0xD0E1 ^ Faction::Enemy.index() as u64);

        // Tick 0: order the US force to assault the French base (FireAtWill + attack-move onto it).
        for &t in &m.troops {
            sim.world.stance[t.index as usize] = Stance::FireAtWill;
        }
        let opening: Vec<Command> = m
            .troops
            .iter()
            .map(|&t| Command::AttackMove { entity: t, target: base_pos })
            .collect();
        sim.step(&opening);

        let alive = |sim: &Sim, e: Entity| sim.world.is_alive(e);
        for _ in 1..max_ticks {
            // The French OPFOR is driven by the scripted commander on its cadence (defending its base).
            let cmds = if sim.tick_count().is_multiple_of(COMMANDER_PERIOD) {
                commander_orders(
                    &sim.world,
                    &sim.territory,
                    &sim.resources,
                    &mut enemy_rng,
                    &CommanderConfig::default(),
                    &[],
                    Faction::Enemy,
                    sim.tick_count(),
                )
            } else {
                Vec::new()
            };
            sim.step(&cmds);
            // "Drives to a result": stop once the whole French garrison is down (the assault broke it).
            if m.garrison.iter().all(|&g| !alive(&sim, g)) {
                break;
            }
        }
        let cleared = m.garrison.iter().filter(|&&g| !alive(&sim, g)).count() as u32;
        (sim.checksum(), cleared)
    }

    /// End-to-end exit criterion (factions-plan WS-D): *"a campaign mission seeded US-vs-FR drives to
    /// a result."* The US assault force engages the French OPFOR garrison and the fight resolves —
    /// and, the lockstep property (invariant #7), two independent runs of the same seeded scene +
    /// scripted input agree bit-for-bit on the final checksum. (A desync between the two armies'
    /// tilted loadouts would surface here as a checksum disagreement, not a silent drift.)
    #[test]
    fn us_vs_fr_seize_drives_to_a_result_with_agreeing_checksums() {
        let (sum_a, cleared_a) = drive_us_vs_fr_seize(2000);
        let (sum_b, cleared_b) = drive_us_vs_fr_seize(2000);
        // Two peers seeding the identical US-vs-FR scene and replaying the identical input land on the
        // same world (the cross-client checksum agreement the matrix enforces).
        assert_eq!(sum_a, sum_b, "the US-vs-FR mission is deterministic across runs (lockstep parity)");
        assert_eq!(cleared_a, cleared_b, "both runs reach the identical result");
        // It actually *reaches a result*: the US assault clears the entire French garrison (the
        // mission progresses to an outcome, not a frozen stalemate).
        let m_strength = {
            let mut probe = fresh();
            seed_seize_mission(&mut probe).garrison.len() as u32
        };
        assert_eq!(cleared_a, m_strength, "the US assault broke the whole French OPFOR garrison");
    }
}

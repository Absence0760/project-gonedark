//! Deterministic balance-metrics harness (D30).
//!
//! This is the objective, regression-testable balance signal that the combat/economy re-tune is
//! measured against — it turns "is the balance good?" into integer/fixed-point numbers the sim
//! produces deterministically, instead of a feel judgement. It scripts a handful of canonical
//! fights/economy runs and reads off, **per tick, from fully-observable sim state**, integer
//! metrics:
//!
//! - alive unit count per faction,
//! - summed current HP per faction (as raw Q16.16 bits — never a float),
//! - per-faction resource purse,
//! - controlled-point count per faction.
//!
//! From those it derives the headline balance numbers: **time-to-kill** (ticks until one side
//! is wiped in a symmetric duel), **equal-cost win/trade** (who survives an equal-resource
//! Rifleman-mass vs Heavy-mass fight, and by how much), **suppression pin-vs-kill** timing
//! (does focus-fire pin a target before it dies?), and the **economy ramp** (resources vs tick).
//!
//! Determinism discipline (invariants #1, #7): everything here is integer / `Fixed` and steps
//! the same `Sim` the checksum CI drives, so each metric series is bit-identical on every arch.
//! Floats appear **only** at the stderr print boundary (seconds = ticks/60 for human reading) —
//! exactly the `--time` pattern — and never touch sim state, so the metrics mode cannot move the
//! stdout checksum stream. The CLI prints the series to **stderr**; stdout stays the untouched
//! `<tick> <checksum>` stream.

use gonedark_core::combat;
use gonedark_core::components::{Army, EntityKind, Faction, Stance, UnitKind, Vec2};
use gonedark_core::economy::{self, unit_cost, Resources, HEAVY_COST, RIFLEMAN_COST};
use gonedark_core::ecs::World;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::Sim;
use gonedark_core::terrain::{Cover, Terrain};
use gonedark_core::territory::ControlPoint;
use gonedark_core::trig::{Angle, ANGLE_FULL};

const HZ: u64 = 60;

fn fx(n: i32) -> Fixed {
    Fixed::from_int(n)
}
fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(fx(x), fx(y))
}

/// Spawn a `kind` unit (with its real [`economy::unit_stats`] table stats) of `faction` at
/// `(x, y)`, set to engage at will. Returns nothing — the metrics read world state, not handles.
fn spawn(sim: &mut Sim, x: i32, y: i32, faction: Faction, kind: UnitKind) {
    let (health, weapon) = economy::unit_stats(kind);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = v(x, y);
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = Stance::FireAtWill;
}

// --- Pure metric extractors (these are the testable seam) ---

/// Number of alive **units** of `faction` (buildings excluded). Stable index scan.
pub fn alive_units(world: &World, faction: Faction) -> u32 {
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

/// Summed current HP of `faction`'s alive units, as **raw Q16.16 bits** (i64 to avoid overflow
/// when many units sum) — never a float, so it is a deterministic cross-arch metric.
pub fn summed_hp_bits(world: &World, faction: Faction) -> i64 {
    let mut sum: i64 = 0;
    for i in 0..world.capacity() {
        if world.is_index_alive(i)
            && world.kind[i] == EntityKind::Unit
            && world.faction[i] == faction
        {
            sum += world.health[i].cur.to_bits() as i64;
        }
    }
    sum
}

/// The first faction whose alive-unit count hit zero is the loser; returns `(tick, winner)`.
/// Drives a symmetric duel to a decision. `None` if neither side wiped within `max_ticks`.
fn run_to_wipe(mut sim: Sim, max_ticks: u64) -> Option<(u64, Option<Faction>)> {
    for t in 1..=max_ticks {
        sim.step(&[]);
        let p = alive_units(&sim.world, Faction::Player);
        let e = alive_units(&sim.world, Faction::Enemy);
        if p == 0 || e == 0 {
            let winner = match (p, e) {
                (0, 0) => None, // simultaneous mutual annihilation (perfect symmetry)
                (0, _) => Some(Faction::Enemy),
                _ => Some(Faction::Player),
            };
            return Some((t, winner));
        }
    }
    None
}

// --- Canonical scenarios (each returns a built, un-stepped Sim) ---

/// Symmetric `n`v`n` open-terrain duel of `kind`, lines `sep` apart on an integer grid. Both
/// sides FireAtWill, no orders — a pure weapons trade for the time-to-kill metric.
pub fn open_duel(n: i32, kind: UnitKind, sep: i32) -> Sim {
    let mut sim = Sim::new(0xD0E1_BA1A);
    for k in 0..n {
        spawn(&mut sim, 0, k * 2, Faction::Player, kind);
        spawn(&mut sim, sep, k * 2, Faction::Enemy, kind);
    }
    sim
}

/// Equal-**resource** Rifleman-mass (Player) vs Heavy-mass (Enemy) trade: each side gets
/// `floor(budget / unit_cost)` bodies, `sep` apart. The headline "no faction dominates" metric.
pub fn equal_cost_trade(budget: i64, sep: i32) -> Sim {
    let mut sim = Sim::new(0xC051_7BA1);
    let nr = (budget / RIFLEMAN_COST) as i32;
    let nh = (budget / HEAVY_COST) as i32;
    for k in 0..nr {
        spawn(&mut sim, 0, k, Faction::Player, UnitKind::Rifleman);
    }
    for k in 0..nh {
        spawn(&mut sim, sep, k, Faction::Enemy, UnitKind::Heavy);
    }
    sim
}

/// Economy-only run from the demo seed purse (`Resources::new(500)`): the Player holds one
/// control point, the Enemy holds none. No combat. For the income-ramp curve.
pub fn economy_only() -> Sim {
    let mut sim = Sim::new(0xEC04_0000);
    sim.resources = Resources::new(500);
    // One point the Player already owns (sits on it so territory_system keeps it secured), plus
    // a lone Player unit parked on the point to hold it.
    let mut pt = ControlPoint::neutral(Vec2::ZERO);
    pt.owner = Faction::Player;
    sim.territory.points.push(pt);
    spawn(&mut sim, 0, 0, Faction::Player, UnitKind::Rifleman);
    sim
}

/// Build the cover-comparison fixture: `n` Player attackers (open ground, FireAtWill) shooting
/// `n` Enemy defenders that **hold fire** while standing in `cover`. Holding the defender's fire
/// isolates the metric to "how long does it take to kill a target in `cover`?", so the wipe-tick
/// scales directly with the cover damage multiplier (the attacker's kill rate is constant; only
/// the defender's effective HP changes). The Player attackers are always in the open.
pub fn cover_duel(n: i32, sep: i32, cover: Cover) -> Sim {
    let mut sim = Sim::new(0xC0FE_5751);
    // Paint a cover patch over the defender column. The grid is shared with combat's cover_at, so
    // a unit standing in a covered cell takes mitigated damage.
    let mut terrain = Terrain::open();
    for k in 0..n {
        let (cx, cy) = terrain.cell_of(v(sep, k * 2));
        terrain.set_cover(cx, cy, cover);
    }
    sim.terrain = terrain;
    for k in 0..n {
        spawn(&mut sim, 0, k * 2, Faction::Player, UnitKind::Rifleman);
        spawn(&mut sim, sep, k * 2, Faction::Enemy, UnitKind::Rifleman);
    }
    // Defenders hold fire: the wipe is the attackers killing the covered defenders, nothing else.
    for i in 0..sim.world.capacity() {
        if sim.world.is_index_alive(i) && sim.world.faction[i] == Faction::Enemy {
            sim.world.stance[i] = Stance::HoldFire;
        }
    }
    sim
}

// --- Derived headline metrics ---

/// Time-to-kill (ticks) of a symmetric open 1v1 of `kind`: how long until the duel resolves.
pub fn ttk_1v1(kind: UnitKind) -> Option<u64> {
    run_to_wipe(open_duel(1, kind, 5), 3000).map(|(t, _)| t)
}

/// Outcome of an equal-cost trade at `sep`: `(end_tick, rifle_survivors, heavy_survivors)`.
pub fn equal_cost_outcome(budget: i64, sep: i32) -> (u64, u32, u32) {
    let mut sim = equal_cost_trade(budget, sep);
    for t in 1..=8000u64 {
        sim.step(&[]);
        let r = alive_units(&sim.world, Faction::Player);
        let h = alive_units(&sim.world, Faction::Enemy);
        if r == 0 || h == 0 {
            return (t, r, h);
        }
    }
    (
        8000,
        alive_units(&sim.world, Faction::Player),
        alive_units(&sim.world, Faction::Enemy),
    )
}

// --- Produced-Tank cost-parity (D30 discipline, wave-2 W7) ---------------------------------------
//
// The produced `UnitKind::Tank` (D65 archetype + the wave-1 armour W1 and ballistic gun W4/D72) was
// shipped with a *playtest BASELINE* cost/stat block flagged "NOT D30-measured". This block MEASURES
// it against the matchups that exist TODAY — tank vs Rifleman, tank vs Heavy, tank vs tank — at the
// standard separations the rest of the harness uses (sep 5 close, sep 9 ranged), and the locking
// tests below pin the result so a stray future edit that re-breaks the balance trips CI.
//
// The load-bearing mechanic: a produced tank is a REAL armoured vehicle (`unit_armor`: front 40 /
// side 16 / rear 8), and ALL existing infantry fire at `penetration == 0`, which bounces every
// armour facet (`2·0 ≤ a` — `combat::facing_penetration_multiplier`). So infantry deal LITERALLY
// ZERO damage to a produced tank, on every facet, regardless of cost or numbers — no equal-cost
// budget lets a Rifleman/Heavy mass crack it. That is not a *cost* imbalance a price tweak can fix;
// it is a rock-paper-scissors GAP, and the intended counter is the dedicated anti-tank unit ([D73],
// added separately by wave-2 W8) — NOT a stat/cost change here. This file's job is to MEASURE + LOCK
// that the tank's price buys what its armour/gun deliver against today's roster, and to document the
// gap so the AT unit lands against a measured baseline rather than a vibe.

/// Spawn a unit with its FULL produced loadout — `unit_stats` health+weapon, the `unit_armor`
/// directional plate, and an explicit `hull_heading` — exactly as the real production path
/// (`economy::economy_system`) spawns it. The base [`spawn`] helper omits `unit_kind`/`armor`
/// (harmless for the unarmoured infantry it was written for, where armour is the no-op default);
/// measuring the armoured Tank REQUIRES them, or the tank would (wrongly) take full damage from
/// penetration-0 fire and resolve no facets. Float-free.
fn spawn_produced(sim: &mut Sim, x: i32, y: i32, faction: Faction, kind: UnitKind, hull: Angle) {
    let (health, weapon) = economy::unit_stats(kind);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.unit_kind[i] = kind;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = v(x, y);
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.armor[i] = economy::unit_armor(kind);
    sim.world.hull_heading[i] = hull;
    sim.world.stance[i] = Stance::FireAtWill;
}

/// Equal-**resource** trade of an `infantry`-mass (Player) vs a produced-Tank-mass (Enemy), `sep`
/// apart, run to a wipe (or 8000-tick cap). An equal budget buys `budget/unit_cost` of each, so this
/// is the produced-tank analogue of [`equal_cost_outcome`]. The Enemy tanks are hull-angled INTO the
/// incoming fire (front toward the Player) — the realistic "face your armour at the threat" default,
/// and irrelevant anyway since the infantry's penetration-0 fire bounces *every* facet. Returns
/// `(end_tick, infantry_survivors, tank_survivors, tank_hp_bits)` — the tank HP bits expose that the
/// armour took the fire for **zero loss** (a deterministic Q16.16 integer, never a float).
pub fn tank_vs_infantry_outcome(infantry: UnitKind, budget: i64, sep: i32) -> (u64, u32, u32, i64) {
    let mut sim = Sim::new(0x7A2C_0B11);
    let ni = (budget / unit_cost(infantry)) as i32;
    let nt = (budget / unit_cost(UnitKind::Tank)) as i32;
    for k in 0..ni {
        // Infantry are unarmoured — heading is a no-op; face +X toward the tanks for tidiness.
        spawn_produced(&mut sim, 0, k, Faction::Player, infantry, Angle(0));
    }
    for k in 0..nt {
        // Tanks at +sep facing −X (toward the Player): front plate INTO the incoming fire.
        spawn_produced(&mut sim, sep, k, Faction::Enemy, UnitKind::Tank, Angle(ANGLE_FULL / 2));
    }
    for t in 1..=8000u64 {
        sim.step(&[]);
        let p = alive_units(&sim.world, Faction::Player);
        let e = alive_units(&sim.world, Faction::Enemy);
        if p == 0 || e == 0 {
            return (t, p, e, summed_hp_bits(&sim.world, Faction::Enemy));
        }
    }
    (
        8000,
        alive_units(&sim.world, Faction::Player),
        alive_units(&sim.world, Faction::Enemy),
        summed_hp_bits(&sim.world, Faction::Enemy),
    )
}

/// A 1v1 produced-tank duel, `sep` apart, with each tank's hull turned by `player_hull` / `enemy_hull`
/// so the test can choose which facet each tank presents. Run to a wipe (or a 4000-tick cap — long
/// enough to drain the slow 6-shell magazine and reload several times). Returns
/// `(end_tick, player_alive, enemy_alive, player_hp_bits, enemy_hp_bits)`.
///
/// Drives the "angle the hull / flank to kill" lesson through the AI auto-resolver: AI tanks are
/// literal executors (invariant #3) — they fire along the bearing to the target and never maneuver
/// to flank — so the *spawn* heading decides the facet, and the duel's outcome is entirely a function
/// of which armour each tank presents.
pub fn tank_duel(sep: i32, player_hull: Angle, enemy_hull: Angle) -> (u64, u32, u32, i64, i64) {
    let mut sim = Sim::new(0x7A2C_D0E1);
    spawn_produced(&mut sim, 0, 0, Faction::Player, UnitKind::Tank, player_hull);
    spawn_produced(&mut sim, sep, 0, Faction::Enemy, UnitKind::Tank, enemy_hull);
    for t in 1..=4000u64 {
        sim.step(&[]);
        let p = alive_units(&sim.world, Faction::Player);
        let e = alive_units(&sim.world, Faction::Enemy);
        if p == 0 || e == 0 {
            return (
                t,
                p,
                e,
                summed_hp_bits(&sim.world, Faction::Player),
                summed_hp_bits(&sim.world, Faction::Enemy),
            );
        }
    }
    (
        4000,
        alive_units(&sim.world, Faction::Player),
        alive_units(&sim.world, Faction::Enemy),
        summed_hp_bits(&sim.world, Faction::Player),
        summed_hp_bits(&sim.world, Faction::Enemy),
    )
}

// --- Cross-faction parity (factions-plan WS-B) ---------------------------------------------------

/// Spawn a `kind` unit of `faction` drawing the **`army`-tilted** loadout
/// ([`economy::unit_stats_for`]) at `(x, y)`, FireAtWill — the per-faction-roster analogue of
/// [`spawn`]. Used by [`cross_faction_equal_cost`] to pit one army's roster against another's.
fn spawn_army(sim: &mut Sim, x: i32, y: i32, faction: Faction, army: Army, kind: UnitKind) {
    let (health, weapon) = economy::unit_stats_for(army, kind);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = v(x, y);
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = Stance::FireAtWill;
}

/// The **mirror-of-roles** equal-cost trade (factions-plan WS-B, the per-faction analogue of D30's
/// unit-parity check): an equal-resource mass of `kind` drawn from `player_army` (Player side) vs
/// the *same* archetype drawn from `enemy_army` (Enemy side), `sep` apart, run to a wipe. Because
/// `unit_cost` is army-independent, an equal budget buys equal counts, so this isolates the per-army
/// stat tilt. Returns `(end_tick, player_survivors, enemy_survivors)`.
///
/// **The fairness signal is swap-invariance** (see `cross_faction_mirror_is_swap_invariant`): a
/// FireAtWill mass trade is a Lanchester square-law snowball that hands the Player side a fixed
/// first-mover edge from index order alone, so the absolute survivor count is *not* the parity
/// metric — the metric is that swapping which army each side fields leaves the outcome unchanged.
/// A fair (power-neutral) roster is invariant under that swap; a power-creeping one is not.
pub fn cross_faction_equal_cost(
    kind: UnitKind,
    budget: i64,
    sep: i32,
    player_army: Army,
    enemy_army: Army,
) -> (u64, u32, u32) {
    let mut sim = Sim::new(0xFAC2_0B11);
    let n = (budget / unit_cost(kind)) as i32;
    for k in 0..n {
        spawn_army(&mut sim, 0, k, Faction::Player, player_army, kind);
        spawn_army(&mut sim, sep, k, Faction::Enemy, enemy_army, kind);
    }
    for t in 1..=8000u64 {
        sim.step(&[]);
        let p = alive_units(&sim.world, Faction::Player);
        let e = alive_units(&sim.world, Faction::Enemy);
        if p == 0 || e == 0 {
            return (t, p, e);
        }
    }
    (
        8000,
        alive_units(&sim.world, Faction::Player),
        alive_units(&sim.world, Faction::Enemy),
    )
}

/// Focus-fire suppression timing: `m` Riflemen firing into a tight enemy **cluster** (three
/// Riflemen packed within [`combat::SUPPRESSION_RADIUS`]). Returns `(pin_tick, kill_tick)` —
/// `pin_tick` is the first tick any *alive* cluster member reaches [`combat::SUPPRESSION_PIN`],
/// `kill_tick` the first tick a cluster member dies (0 if it never happened within budget).
///
/// A **cluster**, not a lone body, because this measures **area suppression** (WS-B): a shot pins
/// the soldiers *near* the impact, not just the one hit. The property it exists to prove —
/// "concentrated fire pins the cluster before it is wiped one-by-one" — only has meaning when the
/// target has neighbours. A lone shooter (`m == 1`) still never pins: one hit per cooldown decays
/// before the next, so suppression never reaches the line and the kill comes by damage.
pub fn focus_fire_pin_kill(m: i32) -> (u64, u64) {
    let mut sim = Sim::new(0x5077_F1AE);
    for k in 0..m {
        spawn(&mut sim, 0, k, Faction::Player, UnitKind::Rifleman);
    }
    // Three enemies packed tight (pairwise distance <= ~1.4 < SUPPRESSION_RADIUS) so every shot into
    // the clump splashes the whole cluster.
    for &(x, y) in &[(5, 0), (5, 1), (6, 0)] {
        spawn(&mut sim, x, y, Faction::Enemy, UnitKind::Rifleman);
    }
    let cluster: Vec<usize> = (0..sim.world.capacity())
        .filter(|&i| sim.world.is_index_alive(i) && sim.world.faction[i] == Faction::Enemy)
        .collect();
    // The cluster HOLDS FIRE (like `cover_duel`'s defenders): isolate the metric to the *incoming*
    // fire's pin-vs-kill timing. With FireAtWill the cluster's 3 guns would wipe a lone (m == 1)
    // attacker before it could shoot back, hiding the lone-shooter-never-pins property we measure.
    for &i in &cluster {
        sim.world.stance[i] = Stance::HoldFire;
    }
    let alive_cluster = |w: &gonedark_core::ecs::World| -> u32 {
        cluster.iter().filter(|&&i| w.is_index_alive(i)).count() as u32
    };
    let start = alive_cluster(&sim.world);
    let mut pin_tick = 0u64;
    let mut kill_tick = 0u64;
    for t in 1..=1200u64 {
        sim.step(&[]);
        if pin_tick == 0
            && cluster.iter().any(|&i| {
                sim.world.is_index_alive(i) && sim.world.suppression[i] >= combat::SUPPRESSION_PIN
            })
        {
            pin_tick = t;
        }
        if kill_tick == 0 && alive_cluster(&sim.world) < start {
            kill_tick = t;
        }
        if alive_cluster(&sim.world) == 0 {
            break;
        }
        // Stop once we know both timings (pin recorded and a death recorded).
        if pin_tick != 0 && kill_tick != 0 {
            break;
        }
    }
    (pin_tick, kill_tick)
}

// --- CLI entry: print the metric series for a named scenario to stderr ---

/// A metrics scenario selectable on the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    OpenDuel,
    CoverDuel,
    EqualCost,
    Economy,
    Summary,
}

impl Metric {
    pub fn parse(token: &str) -> Option<Metric> {
        match token {
            "open-duel" => Some(Metric::OpenDuel),
            "cover-duel" => Some(Metric::CoverDuel),
            "equal-cost" => Some(Metric::EqualCost),
            "economy" => Some(Metric::Economy),
            "summary" => Some(Metric::Summary),
            _ => None,
        }
    }
}

// DISPLAY ONLY — never feed the return value into sim logic or the checksum stream. Floats are
// permitted here solely because this formats a tick count as wall-seconds for stderr/human output
// (mirrors the `--time` pattern in main.rs).
fn secs(t: u64) -> f64 {
    t as f64 / HZ as f64
}

/// Run `metric` for `ticks` ticks, printing the per-tick series (and a final digest) to stderr.
/// Steps the sim but emits **nothing to stdout** — the checksum stream is the caller's job.
pub fn report(metric: Metric, ticks: u64) {
    match metric {
        Metric::OpenDuel => series_duel("open 4v4 rifle", open_duel(4, UnitKind::Rifleman, 5), ticks),
        Metric::CoverDuel => series_duel(
            "4v4 rifle, enemy in Heavy cover",
            cover_duel(4, 5, Cover::Heavy),
            ticks,
        ),
        Metric::EqualCost => series_duel("equal-cost 1000 (rifle vs heavy)", equal_cost_trade(1000, 5), ticks),
        Metric::Economy => series_economy(economy_only(), ticks),
        Metric::Summary => summary(),
    }
}

fn series_duel(name: &str, mut sim: Sim, ticks: u64) {
    eprintln!("# metrics: {name} ({ticks} ticks)");
    eprintln!("# tick  P_alive E_alive  P_hp_bits E_hp_bits");
    for t in 0..ticks {
        if t > 0 {
            sim.step(&[]);
        }
        eprintln!(
            "{t}\t{}\t{}\t{}\t{}",
            alive_units(&sim.world, Faction::Player),
            alive_units(&sim.world, Faction::Enemy),
            summed_hp_bits(&sim.world, Faction::Player),
            summed_hp_bits(&sim.world, Faction::Enemy),
        );
    }
}

fn series_economy(mut sim: Sim, ticks: u64) {
    eprintln!("# metrics: economy ramp from 500 seed, Player holds 1 point ({ticks} ticks)");
    eprintln!("# tick  P_resources E_resources  P_points");
    for t in 0..ticks {
        if t > 0 {
            sim.step(&[]);
        }
        eprintln!(
            "{t}\t{}\t{}\t{}",
            sim.resources.get(Faction::Player),
            sim.resources.get(Faction::Enemy),
            sim.territory.controlled_count(Faction::Player),
        );
    }
}

/// The one-screen digest of every headline metric — the human-readable balance verdict.
fn summary() {
    eprintln!("# balance summary (D66 lethal baseline — ×5 damage over D30)");
    if let Some(t) = ttk_1v1(UnitKind::Rifleman) {
        eprintln!("rifle 1v1 TTK: {t} ticks ({:.1}s)", secs(t));
    }
    if let Some(t) = ttk_1v1(UnitKind::Heavy) {
        eprintln!("heavy 1v1 TTK: {t} ticks ({:.1}s)", secs(t));
    }
    for &(budget, sep) in &[(500i64, 5i32), (1000, 5), (1000, 9)] {
        let (t, r, h) = equal_cost_outcome(budget, sep);
        eprintln!(
            "equal-cost {budget} sep{sep}: ended {t} ticks ({:.1}s), rifle survivors {r}, heavy survivors {h}",
            secs(t)
        );
    }
    for &m in &[1i32, 2, 4] {
        let (pin, kill) = focus_fire_pin_kill(m);
        eprintln!(
            "{m}-on-1 focus: pin at {pin} ({:.1}s), kill at {kill} ({:.1}s)",
            secs(pin),
            secs(kill)
        );
    }
    // Cross-faction parity (factions-plan WS-B): the equal-cost mirror-of-roles trade must be
    // SWAP-INVARIANT — fielding US-vs-FR gives the identical outcome to FR-vs-US and to the Neutral
    // baseline. A `same` verdict means the per-army roster is power-neutral (asymmetry of feel, never
    // power); a `MISMATCH` means a tilt leaked combat power and broke the fairness band.
    eprintln!("# cross-faction parity (WS-B): swap-invariance of the equal-cost mirror trade");
    for kind in [UnitKind::Rifleman, UnitKind::Heavy, UnitKind::Tank] {
        for &sep in &[5i32, 9] {
            let uf = cross_faction_equal_cost(kind, 2000, sep, Army::Us, Army::Fr);
            let fu = cross_faction_equal_cost(kind, 2000, sep, Army::Fr, Army::Us);
            let nn = cross_faction_equal_cost(kind, 2000, sep, Army::Neutral, Army::Neutral);
            let fair = uf == fu && uf == nn;
            eprintln!(
                "{kind:?} sep{sep}: US/FR {uf:?}  FR/US {fu:?}  N/N {nn:?}  [{}]",
                if fair { "fair: swap-invariant" } else { "MISMATCH: roster leaks power" }
            );
        }
    }
    // Produced-tank cost-parity (D30, wave-2 W7): the armoured tank vs today's roster. Infantry fire
    // at penetration 0, so it bounces every facet — the tank wins the equal-cost trade for ZERO loss.
    // The HARD-counter gap is closed by the dedicated AT unit (D73), not a price change.
    eprintln!("# produced-tank cost-parity (D30): equal-resource trades vs today's roster");
    for kind in [UnitKind::Rifleman, UnitKind::Heavy] {
        for &(budget, sep) in &[(360i64, 5i32), (720, 9)] {
            let (t, inf, tanks, hp) = tank_vs_infantry_outcome(kind, budget, sep);
            let untouched = hp == 300 * (Fixed::SCALE as i64) * tanks as i64;
            eprintln!(
                "tank vs {kind:?} budget {budget} sep{sep}: ended {t} ({:.1}s), {kind:?} survivors {inf}, tanks {tanks} [{}]",
                secs(t),
                if untouched { "tanks UNTOUCHED — infantry can't pen" } else { "tanks took damage" },
            );
        }
    }
    let ff = tank_duel(9, Angle(0), Angle(ANGLE_FULL / 2));
    let fl = tank_duel(9, Angle(0), Angle(0));
    eprintln!("tank duel sep9 front/front: {ff:?} (stalemate: pen 18 bounces the 40-front)");
    eprintln!("tank duel sep9 flank(rear): {fl:?} (flank pens the 8-rear — angle the hull / flank to kill)");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Raw Q16.16 bits of one full-health produced Tank (300 HP) — the "took zero damage" yardstick.
    const TANK_FULL_HP_BITS: i64 = 300 * (Fixed::SCALE as i64);

    /// **MEASURED produced-tank cost-parity vs today's roster (D30 discipline, wave-2 W7).** The
    /// headline lock: at *equal resources*, a produced-Tank mass HARD-COUNTERS every existing infantry
    /// archetype — it wipes the cost-equal Rifleman/Heavy mass while taking **literally zero damage**
    /// (its armour bounces their `penetration == 0` fire on every facet — `core` proves the mechanism;
    /// this proves the battlefield outcome). The tank ends at *full* HP with the infantry 0-for, at
    /// both the close (sep 5) and ranged (sep 9) separations, for a single tank (budget 360) and a
    /// two-tank mass (720). This is NOT a cost imbalance a price tweak could fix — no budget buys an
    /// infantry shot that pens — so the cost is held; the intended counter is the dedicated anti-tank
    /// unit ([D73], added separately). Exact ticks pinned so a stray edit that drifts the numbers trips.
    #[test]
    fn produced_tank_hard_counters_infantry_at_equal_cost() {
        // (infantry kind, budget, sep) => (end_tick, infantry_survivors, tank_survivors, tank_hp_bits)
        type Case = (UnitKind, i64, i32, (u64, u32, u32, i64));
        let cases: &[Case] = &[
            // vs RIFLEMAN (cost 100): budget 360 -> 1 tank vs 3 rifles; 720 -> 2 tanks vs 7 rifles.
            (UnitKind::Rifleman, 360, 5, (155, 0, 1, TANK_FULL_HP_BITS)),
            (UnitKind::Rifleman, 360, 9, (157, 0, 1, TANK_FULL_HP_BITS)),
            (UnitKind::Rifleman, 720, 5, (314, 0, 2, 2 * TANK_FULL_HP_BITS)),
            (UnitKind::Rifleman, 720, 9, (315, 0, 2, 2 * TANK_FULL_HP_BITS)),
            // vs HEAVY (cost 220): budget 360 -> 1 tank vs 1 heavy; 720 -> 2 tanks vs 3 heavies.
            (UnitKind::Heavy, 360, 5, (153, 0, 1, TANK_FULL_HP_BITS)),
            (UnitKind::Heavy, 360, 9, (155, 0, 1, TANK_FULL_HP_BITS)),
            (UnitKind::Heavy, 720, 5, (304, 0, 2, 2 * TANK_FULL_HP_BITS)),
            (UnitKind::Heavy, 720, 9, (306, 0, 2, 2 * TANK_FULL_HP_BITS)),
        ];
        for &(kind, budget, sep, expected) in cases {
            let got = tank_vs_infantry_outcome(kind, budget, sep);
            // Direction (the load-bearing balance facts): infantry wiped, tanks all survive, and the
            // tank HP is UNTOUCHED — the armour ate the entire equal-cost volley for nothing.
            assert_eq!(got.1, 0, "{kind:?} budget {budget} sep{sep}: infantry must be wiped");
            assert_eq!(got.2, expected.2, "{kind:?} budget {budget} sep{sep}: every tank survives");
            assert_eq!(
                got.3, expected.3,
                "{kind:?} budget {budget} sep{sep}: tanks take ZERO damage (full HP) — pen-0 infantry bounce"
            );
            // Exact regression pin (deterministic, dev==release): catches a silent drift.
            assert_eq!(got, expected, "{kind:?} budget {budget} sep{sep}: measured cost-parity pin moved");
        }
    }

    /// **MEASURED produced-tank-vs-tank (the "angle the hull / flank to kill" lesson through the AI
    /// auto-resolver, wave-2 W7).** Two AI tanks facing each other head-on cannot crack one another:
    /// the duel-class gun (pen 18) bounces the 40-front (`2·18 = 36 < 40`), so a front/front duel is a
    /// **stalemate** — both alive at full HP when the 4000-tick cap is hit, at every separation. Expose
    /// a tank's rear and it dies fast (pen 18 ≥ 8-rear) while the flanker stays untouched. AI tanks are
    /// literal executors (invariant #3): they never maneuver to flank, so the *presented facet* — set at
    /// spawn — wholly decides the fight. This is the cost-parity floor for the mirror (tank vs tank) and
    /// the reason a tank mass is not self-checking: cracking it needs a flank or a penetrating counter.
    #[test]
    fn produced_tank_duel_is_a_frontal_stalemate_but_a_flank_kills() {
        let toward_enemy = Angle(0); // Player at x=0 faces +X, toward the Enemy
        let toward_player = Angle(ANGLE_FULL / 2); // Enemy at +sep faces −X, toward the Player
        let away_from_player = Angle(0); // Enemy faces +X → its REAR is toward the Player

        for &sep in &[5i32, 9, 13] {
            // FRONT vs FRONT: neither gun pens the other's frontal plate → no damage, runs to the cap.
            let (t, p, e, php, ehp) = tank_duel(sep, toward_enemy, toward_player);
            assert_eq!(
                (t, p, e, php, ehp),
                (4000, 1, 1, TANK_FULL_HP_BITS, TANK_FULL_HP_BITS),
                "sep{sep}: a head-on tank duel must stalemate — both tanks alive at full HP",
            );
            // FLANK: the Player faces the Enemy's exposed rear → Enemy dies, Player untouched.
            let (tf, pf, ef, phpf, ehpf) = tank_duel(sep, toward_enemy, away_from_player);
            assert!(pf == 1 && ef == 0, "sep{sep}: the flanker kills the rear-exposed tank");
            assert_eq!(phpf, TANK_FULL_HP_BITS, "sep{sep}: the flanker takes no return damage");
            assert_eq!(ehpf, 0, "sep{sep}: the flanked tank is destroyed");
            assert!(tf < 4000, "sep{sep}: a flank shot resolves the duel well inside the cap (t={tf})");
        }
        // Exact regression pin for the flank kill at the close separation (deterministic dev==release).
        assert_eq!(
            tank_duel(5, Angle(0), Angle(0)),
            (153, 1, 0, TANK_FULL_HP_BITS, 0),
            "flank-kill measured pin moved",
        );
    }

    /// **Cross-faction parity (factions-plan WS-B, the per-faction analogue of D30's unit-parity
    /// check).** The equal-cost mirror-of-roles trade must be **swap-invariant**: fielding US on the
    /// Player side vs FR on the Enemy side gives the *identical* outcome to fielding FR on the Player
    /// vs US on the Enemy — and the identical outcome to the no-army (Neutral) baseline. That zero
    /// delta is the tightest possible fairness band: army identity contributes **no combat power**,
    /// only feel (the logistics/turret tilt). A power-creeping roster would break this equality.
    ///
    /// Why swap-invariance rather than "even survivor counts": a `FireAtWill` mass trade is a
    /// Lanchester square-law snowball, so the Player side wins by a fixed first-mover margin from
    /// entity index order alone — independent of army. Swapping the armies cancels that artifact and
    /// isolates the army's contribution, which a fair tilt leaves at exactly zero.
    #[test]
    fn cross_faction_mirror_is_swap_invariant() {
        // Budget = 2000 buys a real mass of each archetype (20 rifles / 9 heavies / 5 tanks).
        for kind in [UnitKind::Rifleman, UnitKind::Heavy, UnitKind::Tank] {
            for &sep in &[3i32, 5, 9] {
                let us_vs_fr = cross_faction_equal_cost(kind, 2000, sep, Army::Us, Army::Fr);
                let fr_vs_us = cross_faction_equal_cost(kind, 2000, sep, Army::Fr, Army::Us);
                let baseline = cross_faction_equal_cost(kind, 2000, sep, Army::Neutral, Army::Neutral);
                assert_eq!(
                    us_vs_fr, fr_vs_us,
                    "{kind:?} sep{sep}: swapping armies changed the trade — the roster is NOT power-neutral"
                );
                assert_eq!(
                    us_vs_fr, baseline,
                    "{kind:?} sep{sep}: the per-army trade diverged from the Neutral baseline — the tilt added power"
                );
            }
        }
    }

    /// Reload-pressure stress: a contrived long fight (tanks in heavy cover, ~840 ticks) that runs
    /// long enough for the main gun to **empty and reload mid-fight** — the case where any logistics
    /// tilt bites hardest. Swap-invariance must STILL hold, proving the tilt is power-neutral even
    /// when exercised (the tank tilt is turret-slew-only — cosmetic per invariant #3 — precisely so a
    /// shallow-magazine reload phase can't hand the faster-reloading army a snowball win).
    #[test]
    fn cross_faction_is_swap_invariant_under_reload_pressure() {
        let run = |pa: Army, ea: Army| -> (u64, u32, u32) {
            let mut sim = Sim::new(0x5057_A12D);
            let mut terrain = Terrain::open();
            for k in 0..2 {
                for x in &[0, 7] {
                    let (cx, cy) = terrain.cell_of(v(*x, k * 2));
                    terrain.set_cover(cx, cy, Cover::Heavy);
                }
            }
            sim.terrain = terrain;
            for k in 0..2 {
                spawn_army(&mut sim, 0, k * 2, Faction::Player, pa, UnitKind::Tank);
                spawn_army(&mut sim, 7, k * 2, Faction::Enemy, ea, UnitKind::Tank);
            }
            for t in 1..=12000u64 {
                sim.step(&[]);
                let p = alive_units(&sim.world, Faction::Player);
                let e = alive_units(&sim.world, Faction::Enemy);
                if p == 0 || e == 0 {
                    return (t, p, e);
                }
            }
            (12000, alive_units(&sim.world, Faction::Player), alive_units(&sim.world, Faction::Enemy))
        };
        let us_fr = run(Army::Us, Army::Fr);
        let fr_us = run(Army::Fr, Army::Us);
        let baseline = run(Army::Neutral, Army::Neutral);
        assert_eq!(us_fr, fr_us, "tank reload-pressure trade is not swap-invariant — the tilt carries power");
        assert_eq!(us_fr, baseline, "tank reload-pressure trade diverged from the Neutral baseline");
    }

    /// The tilt is REAL, not a no-op (so the parity above is a genuine "feel without power", not a
    /// trivially-identical roster): each combat archetype's US and FR loadouts differ in their
    /// logistics axis, while every army's Medic and the Neutral baseline stay the shared `unit_stats`.
    #[test]
    fn per_army_loadouts_are_distinct_but_neutral_matches_baseline() {
        for kind in [UnitKind::Rifleman, UnitKind::Heavy, UnitKind::Tank] {
            let us = economy::unit_stats_for(Army::Us, kind).1;
            let fr = economy::unit_stats_for(Army::Fr, kind).1;
            assert_ne!(us, fr, "{kind:?}: US and FR loadouts must differ (a real roster, not a reskin)");
            // The combat-power axes are SHARED (the fairness bound): same damage, cadence, range.
            assert_eq!(us.damage, fr.damage, "{kind:?}: per-shot damage is shared (fairness)");
            assert_eq!(us.cooldown_ticks, fr.cooldown_ticks, "{kind:?}: cadence is shared (fairness)");
            assert_eq!(us.range, fr.range, "{kind:?}: range is shared (fairness)");
            // Neutral is byte-for-byte the shared baseline.
            assert_eq!(economy::unit_stats_for(Army::Neutral, kind), economy::unit_stats(kind));
        }
        // The Medic is shared across every army (no fair combat surface to tilt — see economy.rs).
        for army in [Army::Neutral, Army::Us, Army::Fr] {
            assert_eq!(economy::unit_stats_for(army, UnitKind::Medic), economy::unit_stats(UnitKind::Medic));
        }
    }

    /// A duel stepped twice from scratch yields the identical alive/HP series — the determinism
    /// property the whole metrics suite rests on (mirrors sim-runner's checksum determinism test).
    #[test]
    fn metric_series_is_deterministic() {
        let series = |()| {
            let mut sim = open_duel(4, UnitKind::Rifleman, 5);
            let mut out = Vec::new();
            for _ in 0..200 {
                sim.step(&[]);
                out.push((
                    alive_units(&sim.world, Faction::Player),
                    summed_hp_bits(&sim.world, Faction::Player),
                    summed_hp_bits(&sim.world, Faction::Enemy),
                ));
            }
            out
        };
        assert_eq!(series(()), series(()), "metric extraction must be deterministic");
    }

    /// TTK extraction is deterministic and lands in the D66 *lethal* band (a measured, not feel,
    /// assertion). The ×5 damage re-tune drops the open symmetric 1v1 to ~1.5 s (4 hits) — down
    /// from the old D30 ~8 s attrition. Band is 1 s..=2.5 s at 60 Hz (measured 91 ticks).
    #[test]
    fn rifle_ttk_in_lethal_band() {
        let t = ttk_1v1(UnitKind::Rifleman).expect("1v1 resolves");
        assert_eq!(t, ttk_1v1(UnitKind::Rifleman).unwrap(), "TTK is deterministic");
        // 1 s..=2.5 s band at 60 Hz (D66 target ~1.5 s; measured 91 ticks).
        assert!(
            (HZ..=(5 * HZ / 2)).contains(&t),
            "rifle 1v1 TTK {t} ticks ({:.1}s) outside the lethal 1-2.5s band",
            secs(t)
        );
    }

    /// INTENDED RPS (combat-rebalance-plan WS-A, Q18). The equal-cost Rifleman-vs-Heavy trade is
    /// range-dependent *by design*: the cost-equal Heavy mass out-trades the rifles at point-blank,
    /// the longer-ranged rifles kite and win at range. This reverses the D66 regression-lock
    /// (`equal_cost_outcomes_locked_at_lethal_baseline`): the ×5 lethality re-tune had flattened the
    /// RPS so rifles won at *every* range (heavies 0-for); the Heavy HP/burst re-tune (280/90 →
    /// 300/100) restores the D26/D30 matchup at lethal speed. Asserts the *direction* (who wins,
    /// nobody-0-for) plus the exact measured survivor/tick pins so a stray edit that drifts the
    /// numbers still trips.
    #[test]
    fn heavy_wins_close_rifle_wins_at_range() {
        // CLOSE (sep 5): the Heavy blob trades up at point-blank — heavy survives, rifle 0-for.
        let (t_close, r_close, h_close) = equal_cost_outcome(500, 5);
        assert!(h_close > 0 && r_close == 0, "close (sep5): heavy must win, got rifle {r_close} heavy {h_close}");
        assert_eq!((t_close, r_close, h_close), (98, 0, 1), "close 500: measured RPS pin (post-WS-B suppression)");
        // RANGE (sep 9): the longer-reaching rifles kite — rifle survives, heavy 0-for.
        let (t_range, r_range, h_range) = equal_cost_outcome(1000, 9);
        assert!(r_range > 0 && h_range == 0, "range (sep9): rifle must win, got rifle {r_range} heavy {h_range}");
        assert_eq!((t_range, r_range, h_range), (181, 3, 0), "ranged 1000: measured RPS pin (post-WS-B suppression)");
    }

    /// INTENDED SUPPRESSION (combat-rebalance-plan WS-B, D70, Q18). Area (fire-and-maneuver)
    /// suppression makes concentrated fire **pin a cluster before it is wiped**, restoring the
    /// "concentrate fire to pin" lever the D66 lethal speed had erased. This reverses the D66
    /// regression-lock (`suppression_no_longer_pins_before_kill_at_lethal_speed`): a 4-shooter
    /// volley into a tight cluster splashes the whole clump over `SUPPRESSION_PIN` (now 3/8) before
    /// the first kill, while a lone shooter — one decaying hit per cooldown — still never pins and
    /// resolves by damage. Asserts the *direction* (pin happens, pin precedes kill, lone never pins)
    /// plus the exact measured ticks so a stray edit still trips.
    #[test]
    fn focus_fire_pins_before_kill_but_lone_shooter_never_pins() {
        let (pin4, kill4) = focus_fire_pin_kill(4);
        assert!(pin4 > 0, "4-shooter focus must pin the cluster (got pin tick {pin4})");
        assert!(pin4 < kill4, "the cluster must pin BEFORE the first kill (pin {pin4}, kill {kill4})");
        assert_eq!((pin4, kill4), (1, 31), "4-on-1 cluster: measured pin/kill pin");
        let (pin1, kill1) = focus_fire_pin_kill(1);
        assert_eq!(pin1, 0, "a lone shooter never pins (suppression decays between shots)");
        assert!(kill1 > 0, "a lone shooter still kills by damage");
        assert_eq!((pin1, kill1), (0, 91), "lone shooter: measured pin/kill pin");
    }

    /// Cover materially extends survival: the enemy line standing in Heavy cover (1/4 damage)
    /// must take strictly longer to wipe than the same line in the open — the cover multiplier is
    /// translating into the intended survival swing, not just a number.
    #[test]
    fn heavy_cover_extends_time_to_wipe() {
        let open = run_to_wipe(cover_duel(4, 5, Cover::None), 4000);
        let heavy = run_to_wipe(cover_duel(4, 5, Cover::Heavy), 8000);
        let open_t = open.expect("open duel resolves").0;
        let heavy_t = heavy.expect("cover duel resolves").0;
        assert!(
            heavy_t > open_t,
            "Heavy cover should extend the fight: open {open_t} vs cover {heavy_t}"
        );
    }

    /// Economy ramp: holding one control point roughly TRIPLES income vs holding none, and the
    /// curve is deterministic. Read straight off the resource series.
    #[test]
    fn one_point_triples_income() {
        let mut sim = economy_only();
        let start_p = sim.resources.get(Faction::Player);
        let start_e = sim.resources.get(Faction::Enemy);
        let n = 600u64; // 10 s
        for _ in 0..n {
            sim.step(&[]);
        }
        let gain_p = sim.resources.get(Faction::Player) - start_p;
        let gain_e = sim.resources.get(Faction::Enemy) - start_e;
        // Player held 1 point the whole run: base+per = 3x base = 3x the Enemy's base-only gain.
        assert_eq!(gain_e, (n as i64), "enemy earns base income only (1/tick)");
        assert_eq!(gain_p, 3 * n as i64, "1 point triples income (base+2*per = 3/tick)");
    }
}

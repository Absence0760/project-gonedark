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
use gonedark_core::components::{EntityKind, Faction, Stance, UnitKind, Vec2};
use gonedark_core::economy::{self, Resources, HEAVY_COST, RIFLEMAN_COST};
use gonedark_core::ecs::World;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::Sim;
use gonedark_core::terrain::{Cover, Terrain};
use gonedark_core::territory::ControlPoint;

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

/// Focus-fire suppression timing: `m` Riflemen on one target Rifleman. Returns
/// `(pin_tick, kill_tick)` — `pin_tick` is when the target first reaches
/// [`combat::SUPPRESSION_PIN`], `kill_tick` when it dies (0 if it never happened within budget).
pub fn focus_fire_pin_kill(m: i32) -> (u64, u64) {
    let mut sim = Sim::new(0x5077_F1AE);
    for k in 0..m {
        spawn(&mut sim, 0, k, Faction::Player, UnitKind::Rifleman);
    }
    spawn(&mut sim, 5, 0, Faction::Enemy, UnitKind::Rifleman);
    let target = (0..sim.world.capacity())
        .find(|&i| sim.world.is_index_alive(i) && sim.world.faction[i] == Faction::Enemy)
        .expect("target spawned");
    let mut pin_tick = 0u64;
    let mut kill_tick = 0u64;
    for t in 1..=1200u64 {
        sim.step(&[]);
        if pin_tick == 0
            && sim.world.is_index_alive(target)
            && sim.world.suppression[target] >= combat::SUPPRESSION_PIN
        {
            pin_tick = t;
        }
        if !sim.world.is_index_alive(target) {
            kill_tick = t;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// MEASURED-BASELINE LOCK (D66). The equal-cost Rifleman-vs-Heavy outcomes are pinned at their
    /// current measured values so a stray edit that shifts them trips a test — NOT an assertion of
    /// intended balance.
    ///
    /// KNOWN REGRESSION the lethality re-tune introduced: at ~1.5 s kill speed the Rifleman mass's
    /// body-count + faster cadence dominate the Heavy mass at *every* range (heavies wiped 0-for),
    /// collapsing the D26/D30 range-dependent rock-paper-scissors. The intended "heavies win close,
    /// rifles kite at range" property needs a Heavy re-tune AT lethal speed — tracked as an open
    /// question (Q on lethal-speed re-tune, `docs/open-questions.md`). This test guards that the
    /// numbers don't drift *silently* before that re-tune lands.
    #[test]
    fn equal_cost_outcomes_locked_at_lethal_baseline() {
        // close (sep 5), seed-purse budget: rifles win 2-for currently (was: heavies win).
        assert_eq!(equal_cost_outcome(500, 5), (151, 2, 0), "close 500: measured lethal baseline");
        // at range (sep 9), larger budget: rifles win 6-for (range advantage holds in the same dir).
        assert_eq!(equal_cost_outcome(1000, 9), (121, 6, 0), "ranged 1000: measured lethal baseline");
    }

    /// MEASURED-BASELINE LOCK (D66). At lethal kill speed the per-*hit* suppression model
    /// (`combat::SUPPRESSION_PER_HIT`) no longer bites: the target dies before suppression can
    /// reach `SUPPRESSION_PIN`, so focus-fire NEVER pins before the kill (pin tick 0 = never).
    /// This is the model gap a per-near-miss "fire-and-maneuver" suppression rework would close —
    /// tracked as an open question. Lock the current reality so it's a conscious change, not a
    /// silent drift, when that rework happens.
    #[test]
    fn suppression_no_longer_pins_before_kill_at_lethal_speed() {
        let (pin4, kill4) = focus_fire_pin_kill(4);
        assert_eq!(pin4, 0, "lethal speed: 4-on-1 kills before suppression can pin");
        assert!(kill4 > 0, "4-on-1 still kills (near-instantly)");
        let (pin1, kill1) = focus_fire_pin_kill(1);
        assert_eq!(pin1, 0, "a lone shooter never pins");
        assert!(kill1 > 0, "a lone shooter kills by damage");
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

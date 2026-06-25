//! Headless deterministic driver (invariant #7, docs/phase-1-plan.md §6, docs/phase-3-plan.md
//! §"Workstream A").
//!
//! Plays a fixed input script and prints a per-tick `<tick> <checksum>` stream. CI runs this
//! on every target in the matrix (`x86_64-*`, `aarch64-*`) and diffs the streams; any
//! divergence is a desync — a real bug, never silenced by narrowing the matrix.
//!
//! Two scenarios, both fully deterministic (fixed-point/integer, seeded RNG, stable spawn
//! order) so each stream is bit-identical on every arch:
//!
//! - **`phase2`** (default) — the Phase 2 game-systems smoke: two opposing rifle squads fight,
//!   a player camp is built and queues a unit that finishes mid-run, one unit patrols, one is
//!   embodied, two control points contested. This is the existing CI baseline; its stream is
//!   unchanged by the Phase 3 work.
//! - **`stress` / `stress:<n>`** — a Phase 3 scaling scene: `n` riflemen (default 200) in two
//!   opposing lines, several camps producing into queues, contested points, mixed orders, one
//!   embodied. Exercises the per-tick systems at size so profiling (`--time`) reflects the real
//!   200-unit cost and the determinism gate covers the sim at scale.
//!
//! Usage: `gonedark-sim-runner [ticks] [scenario] [--time] [--metrics[=<which>]]`
//!   (defaults: 300 ticks, `phase2`)
//!   - `--time` prints per-tick wall-clock stats (min/median/p99/max ms) to **stderr**; the
//!     `<tick> <checksum>` stream on stdout is unchanged. Timing is host-side only (`Instant`)
//!     and never touches sim state, so it cannot move the checksum.
//!   - `--metrics[=<which>]` runs the deterministic balance-metrics harness ([`metrics`]) and
//!     prints the metric series (or, with `summary`, the headline digest) to **stderr** — the
//!     objective signal the D30 combat/economy re-tune was tuned against. `<which>` is one of
//!     `open-duel` / `cover-duel` / `equal-cost` / `economy` / `summary` (default `summary`).
//!     Like `--time`, it touches stdout not at all, so determinism is unaffected; it runs its own
//!     canonical fights instead of the `phase2`/`stress` scenario and then exits.

mod metrics;

use std::collections::BTreeMap;
use std::time::Instant;

use gonedark_core::components::{BuildingKind, EntityKind, Faction, Order, Stance, UnitKind, Vec2};
use gonedark_core::economy::{self, Resources};
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::{Command, Sim};
use gonedark_core::territory::ControlPoint;

fn fx(n: i32) -> Fixed {
    Fixed::from_int(n)
}

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(fx(x), fx(y))
}

/// Spawn a Rifleman of `faction` at `(x, y)`, set to engage at will, and return its handle.
fn spawn_rifleman(sim: &mut Sim, x: i32, y: i32, faction: Faction) -> Entity {
    let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = v(x, y);
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = Stance::FireAtWill;
    e
}

/// A built scenario: the seeded sim plus the scripted commands to apply, keyed by the tick they
/// execute on. A `BTreeMap` keeps lookup deterministic; commands within a tick keep insertion
/// order (the stable application order `Sim::step` relies on).
struct Scenario {
    sim: Sim,
    scripted: BTreeMap<u64, Vec<Command>>,
}

impl Scenario {
    fn commands_for(&self, tick: u64) -> &[Command] {
        self.scripted.get(&tick).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Which scene to run. Parsed from the CLI scenario token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Phase2,
    /// `n` total units across both factions.
    Stress(u32),
}

impl Which {
    fn parse(token: &str) -> Option<Which> {
        match token {
            "phase2" => Some(Which::Phase2),
            "stress" => Some(Which::Stress(200)),
            other => other
                .strip_prefix("stress:")
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n >= 2)
                .map(Which::Stress),
        }
    }
}

fn build(which: Which) -> Scenario {
    match which {
        Which::Phase2 => build_phase2(),
        Which::Stress(n) => build_stress(n),
    }
}

/// The Phase 2 smoke scene. Reproduces the original sim-runner scenario exactly so its CI
/// checksum stream is unchanged.
fn build_phase2() -> Scenario {
    let mut sim = Sim::new(0x00C0FFEE);
    sim.resources = Resources::new(500);

    sim.territory.points.push(ControlPoint::neutral(Vec2::ZERO));
    sim.territory.points.push(ControlPoint::neutral(v(20, -20)));

    let p1 = spawn_rifleman(&mut sim, -5, 0, Faction::Player);
    let p2 = spawn_rifleman(&mut sim, -5, 3, Faction::Player);
    let _e1 = spawn_rifleman(&mut sim, 5, 0, Faction::Enemy);
    let _e2 = spawn_rifleman(&mut sim, 5, 3, Faction::Enemy);

    let camp = economy::build(
        &mut sim.world,
        &mut sim.resources,
        Faction::Player,
        BuildingKind::Camp,
        v(-20, 20),
    )
    .expect("camp affordable at 500 resources");

    let mut scripted = BTreeMap::new();
    // Tick 1: the order vocabulary — attack-move, patrol+retreat trigger, embody.
    scripted.insert(
        1,
        vec![
            Command::AttackMove {
                entity: p1,
                target: v(5, 0),
            },
            Command::SetOrder {
                entity: p2,
                order: Order::Patrol {
                    a: v(-5, 3),
                    b: v(-5, -8),
                    toward_b: true,
                },
            },
            Command::SetRetreatThreshold {
                entity: p2,
                fraction: Fixed::from_ratio(1, 3),
            },
            Command::Embody { entity: p1 },
        ],
    );
    // Tick 150: the finished camp queues a unit.
    scripted.insert(
        150,
        vec![Command::QueueProduction {
            camp,
            unit: UnitKind::Rifleman,
        }],
    );

    Scenario { sim, scripted }
}

/// The Phase 3 scaling scene: `n` riflemen in two opposing lines, several producing camps,
/// contested points, mixed orders, one embodied. Everything is integer/fixed-point and spawn
/// order is stable, so it is deterministic and bit-identical across arch.
fn build_stress(n: u32) -> Scenario {
    let mut sim = Sim::new(0x5712E55);
    // Plenty of purse so the camps actually produce at scale.
    sim.resources = Resources::new(100_000);

    // A spread of contested control points so `territory_system` scales too.
    for k in 0..4i32 {
        sim.territory
            .points
            .push(ControlPoint::neutral(v(k * 12 - 18, 0)));
    }

    // Two facing lines, `per` units each, on an integer grid. Player on the left (x<0),
    // Enemy on the right (x>0), rows wrapped at 20 wide so dense clusters form — the O(n^2)
    // target-acquisition and per-unit flow-field costs this scene exists to surface.
    let per = (n / 2) as i32;
    let mut player: Vec<Entity> = Vec::new();
    let mut enemy: Vec<Entity> = Vec::new();
    for k in 0..per {
        let col = k % 20;
        let row = k / 20;
        let px = -40 + col;
        let ex = 40 - col;
        let y = -row * 2;
        player.push(spawn_rifleman(&mut sim, px, y, Faction::Player));
        enemy.push(spawn_rifleman(&mut sim, ex, y, Faction::Enemy));
    }

    // A few player camps that finish and produce, exercising the economy/spawn path (and the
    // free-list recycling, a determinism-sensitive path) at scale.
    let mut camps: Vec<Entity> = Vec::new();
    for k in 0..4i32 {
        if let Some(c) = economy::build(
            &mut sim.world,
            &mut sim.resources,
            Faction::Player,
            BuildingKind::Camp,
            v(-50, k * 6 - 9),
        ) {
            camps.push(c);
        }
    }

    let mut scripted: BTreeMap<u64, Vec<Command>> = BTreeMap::new();
    let mut tick1: Vec<Command> = Vec::new();

    // Most player units attack-move into the enemy line; every fifth patrols a short beat so
    // `order_system`'s patrol/flow-field path is covered at scale.
    for (k, &e) in player.iter().enumerate() {
        if k % 5 == 0 {
            let base = v(-40 + (k as i32 % 20), -(k as i32 / 20) * 2);
            tick1.push(Command::SetOrder {
                entity: e,
                order: Order::Patrol {
                    a: base,
                    b: Vec2::new(base.x, base.y.wrapping_add(fx(4))),
                    toward_b: true,
                },
            });
        } else {
            tick1.push(Command::AttackMove {
                entity: e,
                target: v(40, 0),
            });
        }
    }
    // Enemy units hold their stance and attack-move back, so both sides actually close.
    for &e in &enemy {
        tick1.push(Command::AttackMove {
            entity: e,
            target: v(-40, 0),
        });
    }
    // Exactly one embodied unit (exercises the embodied skip-paths at scale).
    if let Some(&first) = player.first() {
        tick1.push(Command::Embody { entity: first });
    }
    scripted.insert(1, tick1);

    // Stagger production so spawns trickle in across the run rather than all at once.
    for (k, &camp) in camps.iter().enumerate() {
        scripted.insert(
            120 + k as u64 * 15,
            vec![Command::QueueProduction {
                camp,
                unit: UnitKind::Rifleman,
            }],
        );
    }

    Scenario { sim, scripted }
}

/// Run `scenario` for `ticks` ticks, printing the `<tick> <checksum>` stream to stdout. Returns
/// per-tick durations (micros) when `timed`, else an empty vec.
fn run(mut scenario: Scenario, ticks: u64, timed: bool) -> Vec<u128> {
    let mut durations: Vec<u128> = if timed {
        Vec::with_capacity(ticks as usize)
    } else {
        Vec::new()
    };

    emit(&scenario.sim); // initial state (tick 0)
    for t in 1..ticks {
        let cmds = scenario.commands_for(t).to_vec();
        if timed {
            let start = Instant::now();
            scenario.sim.step(&cmds);
            durations.push(start.elapsed().as_micros());
        } else {
            scenario.sim.step(&cmds);
        }
        emit(&scenario.sim);
    }

    eprintln!(
        "final tick {} checksum {:016x}",
        scenario.sim.tick_count(),
        scenario.sim.checksum()
    );
    durations
}

fn main() {
    // `ticks` is the first non-flag positional (back-compat: existing callers pass just ticks);
    // `scenario` is the second. `--time` may appear anywhere.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let timed = args.iter().any(|a| a == "--time");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();

    let ticks: u64 = positional
        .first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);

    // `--metrics[=<which>]` is a self-contained balance-harness mode: it runs its own canonical
    // fights, prints the metric series/digest to stderr, and exits without touching the stdout
    // checksum stream (so it can never affect determinism — like `--time`).
    if let Some(metric_arg) = args.iter().find(|a| a.starts_with("--metrics")) {
        let which = metric_arg
            .strip_prefix("--metrics=")
            .map(|w| metrics::Metric::parse(w).unwrap_or_else(|| fatal_metric(w)))
            .unwrap_or(metrics::Metric::Summary);
        metrics::report(which, ticks);
        return;
    }

    let which = positional
        .get(1)
        .map(|s| s.as_str())
        .map(|s| Which::parse(s).unwrap_or_else(|| fatal_scenario(s)))
        .unwrap_or(Which::Phase2);

    let scenario = build(which);
    let durations = run(scenario, ticks, timed);

    if timed {
        report_timing(which, &durations);
    }
}

fn fatal_scenario(s: &str) -> ! {
    eprintln!("unknown scenario {s:?}; expected `phase2`, `stress`, or `stress:<n>`");
    std::process::exit(2);
}

fn fatal_metric(s: &str) -> ! {
    eprintln!(
        "unknown metric {s:?}; expected `open-duel`, `cover-duel`, `equal-cost`, `economy`, or `summary`"
    );
    std::process::exit(2);
}

fn emit(sim: &Sim) {
    println!("{} {:016x}", sim.tick_count(), sim.checksum());
}

/// Print per-tick wall-clock distribution to stderr. Host-side only; the stdout checksum stream
/// is untouched, so timing can never affect determinism.
fn report_timing(which: Which, durations: &[u128]) {
    if durations.is_empty() {
        eprintln!("--time: no ticks measured");
        return;
    }
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let pct = |p: usize| sorted[(n.saturating_sub(1) * p) / 100];
    let sum: u128 = sorted.iter().sum();
    let us = |u: u128| u as f64 / 1000.0;
    eprintln!(
        "timing {which:?} over {n} ticks (ms): min {:.3} median {:.3} p99 {:.3} max {:.3} mean {:.3}",
        us(sorted[0]),
        us(pct(50)),
        us(pct(99)),
        us(sorted[n - 1]),
        us(sum / n as u128),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a freshly-built scenario `ticks` ticks and return the final checksum. Pure-core,
    /// no I/O — the deterministic property we assert against.
    fn final_checksum(which: Which, ticks: u64) -> u64 {
        let mut scenario = build(which);
        for t in 1..ticks {
            let cmds = scenario.commands_for(t).to_vec();
            scenario.sim.step(&cmds);
        }
        scenario.sim.checksum()
    }

    fn full_stream(which: Which, ticks: u64) -> Vec<u64> {
        let mut scenario = build(which);
        let mut stream = vec![scenario.sim.checksum()];
        for t in 1..ticks {
            let cmds = scenario.commands_for(t).to_vec();
            scenario.sim.step(&cmds);
            stream.push(scenario.sim.checksum());
        }
        stream
    }

    #[test]
    fn scenario_token_parsing() {
        assert_eq!(Which::parse("phase2"), Some(Which::Phase2));
        assert_eq!(Which::parse("stress"), Some(Which::Stress(200)));
        assert_eq!(Which::parse("stress:50"), Some(Which::Stress(50)));
        assert_eq!(Which::parse("stress:1"), None); // need >= 2 for two factions
        assert_eq!(Which::parse("stress:0"), None);
        assert_eq!(Which::parse("nope"), None);
        assert_eq!(Which::parse("stress:abc"), None);
    }

    #[test]
    fn phase2_is_deterministic() {
        // Same scene built twice must produce the identical stream — the core invariant the CI
        // matrix diffs across arch (invariant #7), asserted here on one arch as a fast guard.
        assert_eq!(
            full_stream(Which::Phase2, 300),
            full_stream(Which::Phase2, 300)
        );
    }

    #[test]
    fn stress_is_deterministic() {
        // 200-unit scene: identical across two independent builds.
        assert_eq!(
            full_stream(Which::Stress(200), 120),
            full_stream(Which::Stress(200), 120)
        );
    }

    #[test]
    fn stress_runs_at_size() {
        // The 200-unit scene actually spawns ~200 units and advances without panicking, and the
        // checksum evolves (the sim is doing work, not frozen).
        let start = build(Which::Stress(200));
        let alive = (0..start.sim.world.capacity())
            .filter(|&i| start.sim.world.is_index_alive(i))
            .count();
        assert!(alive >= 200, "expected >=200 entities, got {alive}");
        assert_ne!(
            final_checksum(Which::Stress(200), 200),
            full_stream(Which::Stress(200), 1)[0],
            "checksum should change as the sim advances"
        );
    }

    #[test]
    fn stress_scales_with_count() {
        // A different unit count is a genuinely different scene (guards against the count arg
        // being silently ignored).
        assert_ne!(
            final_checksum(Which::Stress(50), 100),
            final_checksum(Which::Stress(200), 100)
        );
    }
}

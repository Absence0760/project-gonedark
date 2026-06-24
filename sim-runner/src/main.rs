//! Headless deterministic driver (invariant #7, docs/phase-1-plan.md §6).
//!
//! Plays a fixed input script and prints a per-tick `<tick> <checksum>` stream. CI runs this
//! on every target in the matrix (`x86_64-*`, `aarch64-*`) and diffs the streams; any
//! divergence is a desync — a real bug, never silenced by narrowing the matrix.
//!
//! The Phase 2 scenario exercises the full game-systems surface so the checksum actually folds
//! it (otherwise the determinism gate would be blind to combat/territory/economy): two opposing
//! rifle squads fight across an open field, a player camp is built and queues a unit that
//! finishes mid-run, one unit patrols, one is embodied, and two control points are contested.
//! Everything is fixed-point/integer, so the stream is bit-identical on every arch.
//!
//! Usage: `gonedark-sim-runner [ticks]`  (default 300)

use gonedark_core::components::{EntityKind, Faction, Stance, UnitKind, Vec2};
use gonedark_core::ecs::Entity;
use gonedark_core::economy::{self, Resources};
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

fn main() {
    let ticks: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);

    let mut sim = Sim::new(0x00C0FFEE);

    // Per-faction starting purse so the economy has something to spend.
    sim.resources = Resources::new(500);

    // Two contested control points (one central, one off to the corner).
    sim.territory.points.push(ControlPoint::neutral(Vec2::ZERO));
    sim.territory.points.push(ControlPoint::neutral(v(20, -20)));

    // Two opposing rifle squads within weapon range of each other.
    let p1 = spawn_rifleman(&mut sim, -5, 0, Faction::Player);
    let p2 = spawn_rifleman(&mut sim, -5, 3, Faction::Player);
    let _e1 = spawn_rifleman(&mut sim, 5, 0, Faction::Enemy);
    let _e2 = spawn_rifleman(&mut sim, 5, 3, Faction::Enemy);

    // A player camp under construction; it will finish (~tick 120) and later produce a unit.
    let camp = economy::build(&mut sim.world, &mut sim.resources, Faction::Player, gonedark_core::components::BuildingKind::Camp, v(-20, 20))
        .expect("camp affordable at 500 resources");

    emit(&sim);

    // Tick 1: issue the order vocabulary — attack-move, patrol+retreat trigger, embody.
    sim.step(&[
        Command::AttackMove {
            entity: p1,
            target: v(5, 0),
        },
        Command::SetOrder {
            entity: p2,
            order: gonedark_core::components::Order::Patrol {
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
    ]);
    emit(&sim);

    for t in 2..ticks {
        // Once the camp has finished building, queue a unit (fires once, at tick 150).
        if t == 150 {
            sim.step(&[Command::QueueProduction {
                camp,
                unit: UnitKind::Rifleman,
            }]);
        } else {
            sim.step(&[]);
        }
        emit(&sim);
    }

    eprintln!(
        "final tick {} checksum {:016x}",
        sim.tick_count(),
        sim.checksum()
    );
}

fn emit(sim: &Sim) {
    println!("{} {:016x}", sim.tick_count(), sim.checksum());
}

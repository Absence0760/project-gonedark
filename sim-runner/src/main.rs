//! Headless deterministic driver (invariant #7, docs/phase-1-plan.md §6).
//!
//! Plays a fixed input script and prints a per-tick `<tick> <checksum>` stream. CI runs
//! this on every target in the matrix (`x86_64-*`, `aarch64-*`) and diffs the streams; any
//! divergence is a desync — a real bug, never silenced by narrowing the matrix.
//!
//! Usage: `gonedark-sim-runner [ticks]`  (default 300)

use gonedark_core::components::Vec2;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::{Command, Sim};

fn main() {
    let ticks: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);

    let mut sim = Sim::new(0x00C0FFEE);
    let unit = sim.world.spawn();
    let target = Vec2::new(Fixed::from_int(10), Fixed::from_int(5));

    // tick 0 baseline, then issue the move order, then free-run.
    emit(&sim);
    sim.step(&[Command::Move {
        entity: unit,
        target,
    }]);
    emit(&sim);
    for _ in 1..ticks {
        sim.step(&[]);
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

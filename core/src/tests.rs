//! Core determinism + math tests. These run in CI on every target in the matrix
//! (docs/phase-1-plan.md §6); a cross-arch divergence shows up as a checksum mismatch.

use crate::components::Vec2;
use crate::fixed::Fixed;
use crate::sim::{Command, Sim};
use crate::trig::{self, Angle, ANGLE_FULL};

#[test]
fn fixed_arithmetic() {
    assert_eq!((Fixed::from_int(2) * Fixed::from_int(3)).to_int(), 6);
    assert_eq!(Fixed::from_int(7) / Fixed::from_int(2), Fixed::from_ratio(7, 2));
    assert_eq!(Fixed::ONE + Fixed::ONE, Fixed::from_int(2));
    assert_eq!(Fixed::from_int(-3).abs(), Fixed::from_int(3));
    assert_eq!(Fixed::from_ratio(1, 2), Fixed::HALF);
}

#[test]
fn sqrt_exact_squares() {
    assert_eq!(trig::sqrt(Fixed::from_int(4)), Fixed::from_int(2));
    assert_eq!(trig::sqrt(Fixed::from_int(9)), Fixed::from_int(3));
    assert_eq!(trig::sqrt(Fixed::from_int(144)), Fixed::from_int(12));
    assert_eq!(trig::sqrt(Fixed::ZERO), Fixed::ZERO);
}

#[test]
fn sin_cos_landmarks() {
    let tol = Fixed::from_ratio(1, 1000);
    assert_eq!(trig::sin(Angle(0)), Fixed::ZERO);
    assert!((trig::sin(Angle(ANGLE_FULL / 4)) - Fixed::ONE).abs() <= tol);
    assert!((trig::cos(Angle(0)) - Fixed::ONE).abs() <= tol);
    assert!((trig::sin(Angle(ANGLE_FULL / 2))).abs() <= tol);
}

/// A fixed input script: spawn one unit, order it to (10, 5), let it run 200 ticks.
fn scripted_sim() -> Sim {
    let mut sim = Sim::new(0x00C0FFEE);
    let e = sim.world.spawn();
    let target = Vec2::new(Fixed::from_int(10), Fixed::from_int(5));
    sim.step(&[Command::Move { entity: e, target }]);
    for _ in 0..200 {
        sim.step(&[]);
    }
    sim
}

#[test]
fn deterministic_replay() {
    // Same script, run twice → bit-identical state every way we can observe it.
    let a = scripted_sim();
    let b = scripted_sim();
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(a.tick_count(), b.tick_count());
}

#[test]
fn literal_executor_reaches_target() {
    let sim = scripted_sim();
    let target = Vec2::new(Fixed::from_int(10), Fixed::from_int(5));
    let p = sim.world.pos[0];
    assert!((p - target).len_sq() <= Fixed::from_ratio(1, 16));
}

#[test]
fn embodied_unit_ignores_orders() {
    // A possessed unit is driven by player input, so the order executor must not move it.
    let mut sim = Sim::new(1);
    let e = sim.world.spawn();
    sim.step(&[
        Command::Move {
            entity: e,
            target: Vec2::new(Fixed::from_int(50), Fixed::ZERO),
        },
        Command::Embody { entity: e },
    ]);
    let before = sim.world.pos[e.index as usize];
    for _ in 0..10 {
        sim.step(&[]);
    }
    assert_eq!(sim.world.pos[e.index as usize], before);
}

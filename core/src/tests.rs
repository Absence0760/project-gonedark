//! Core determinism + math tests. These run in CI on every target in the matrix
//! (docs/phase-1-plan.md §6); a cross-arch divergence shows up as a checksum mismatch.

use crate::components::Vec2;
use crate::fixed::Fixed;
use crate::flow_field::FlowField;
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
fn flow_field_is_deterministic() {
    // Building the same field twice must yield bit-identical sampled directions at every
    // probe point — the whole point of a fixed-point, fixed-iteration-order field.
    let goal = Vec2::new(Fixed::from_int(12), Fixed::from_int(-7));
    let a = FlowField::build(goal);
    let b = FlowField::build(goal);
    let probes = [
        Vec2::ZERO,
        Vec2::new(Fixed::from_int(-30), Fixed::from_int(20)),
        Vec2::new(Fixed::from_int(40), Fixed::from_int(40)),
        Vec2::new(Fixed::from_int(-50), Fixed::from_int(-50)),
        Vec2::new(Fixed::from_int(11), Fixed::from_int(-7)),
        // Out-of-grid positions must clamp identically on both builds.
        Vec2::new(Fixed::from_int(9000), Fixed::from_int(-9000)),
    ];
    for p in probes {
        assert_eq!(a.sample(p), b.sample(p));
    }
}

#[test]
fn flow_field_points_toward_goal() {
    // From the lower-left, the downhill direction must have a positive component toward a
    // goal that sits up and to the right. Open field ⇒ the field points at the goal.
    let goal = Vec2::new(Fixed::from_int(20), Fixed::from_int(15));
    let field = FlowField::build(goal);
    let from = Vec2::new(Fixed::from_int(-20), Fixed::from_int(-15));
    let dir = field.sample(from);
    assert!(dir.x > Fixed::ZERO, "should steer +x toward goal, got {dir:?}");
    assert!(dir.y > Fixed::ZERO, "should steer +y toward goal, got {dir:?}");

    // Sampling at the goal cell aims straight at the true goal centre, which is within the
    // same cell — so the residual direction is tiny (well under one step).
    let at_goal = field.sample(goal);
    assert!(at_goal.len_sq() <= Fixed::ONE);
}

#[test]
fn flow_field_drives_unit_to_target() {
    // A unit driven purely by the flow field must still reach its order's target — the
    // same contract as the straight-line stub, now through real field sampling.
    let mut sim = Sim::new(0xF10F1E1D);
    let e = sim.world.spawn();
    let target = Vec2::new(Fixed::from_int(-18), Fixed::from_int(23));
    sim.step(&[Command::Move { entity: e, target }]);
    for _ in 0..400 {
        sim.step(&[]);
    }
    let p = sim.world.pos[e.index as usize];
    assert!(
        (p - target).len_sq() <= Fixed::from_ratio(1, 16),
        "unit stalled at {p:?}, target {target:?}"
    );
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

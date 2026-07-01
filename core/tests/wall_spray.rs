//! "Spray a magazine at a wall" — the deterministic aim/scatter half of the scenario, as an
//! integration test over the public sim API (no floats: invariant #1).
//!
//! This is the sim-side of the idea "stand a shooter in front of a wall, empty a magazine, and
//! check where the rounds land / how the pattern moves when you re-aim." It validates the
//! *ballistics* (aim → where the round goes on the wall); the *visual* half — "pan left and you see
//! the left of the wall" — is a render/camera assertion that belongs in the `viz-runner` pixel
//! harness (it reads back the embodied frame), not here.
//!
//! Two design facts this test pins down (and demonstrates the difference between):
//!   * A **rifleman** is a pinpoint hitscan — `dispersion == 0`, so `scatter_dir` returns the aim
//!     UNCHANGED (see `core::dispersion`): every round of the magazine lands on the SAME spot, and
//!     panning the aim walks that spot across the wall. Infantry never "sprays randomly".
//!   * A **tank gun** blooms (`dispersion > 0`): the rounds scatter across a bounded, *deterministic*
//!     patch of the wall — the literal "random places", but seeded so every peer sees the identical
//!     pattern (invariant #7). That patch re-centres when the aim shifts.
//!
//! The wall is modelled as a vertical plane `WALL_X` metres downrange; the shooter sits at the
//! origin aiming roughly +X. We ray-cast each (possibly scattered) aim to that plane and read the
//! `y` where it lands — "how far along the wall" the round hit. All fixed-point.

use gonedark_core::components::Vec2;
use gonedark_core::dispersion::{scatter_dir, DISPERSION_MAX};
use gonedark_core::fixed::Fixed;
use gonedark_core::rng::Rng;
use gonedark_core::trig::{self, Angle};

/// Downrange distance to the wall plane (metres). Comfortably inside a rifle's range.
const WALL_X: i32 = 20;
/// Half-height/width of the wall panel (metres). The full-bloom cone at `WALL_X` throws a round at
/// most ~2 m off-centre (a ±5.6° cone over 20 m), so a ±4 m panel always contains the spray.
const WALL_HALF: i32 = 4;
/// A magazine's worth of rounds (matches the 30-round default in `core::resupply`).
const MAG: usize = 30;
/// How far we pan the aim to one side between bursts, in angle-units (`65536` = full turn). ~5.6°,
/// which walks the point of impact ~2 m along the wall at `WALL_X`.
const PAN: i32 = 1024;

/// Where a shot aimed along `heading` (and perturbed by `dispersion`, drawing from `rng`) lands on
/// the wall: the `y` coordinate at the plane `x = WALL_X`. `+y` is one way along the wall, `-y` the
/// other, `0` is dead centre. Fixed-point ray/plane intersection: `t = WALL_X / dir.x`, `y = dir.y·t`.
fn impact_y(heading: Angle, dispersion: Fixed, rng: &mut Rng) -> Fixed {
    let aim = Vec2::new(trig::cos(heading), trig::sin(heading));
    let dir = scatter_dir(aim, dispersion, rng);
    // dir.x stays > 0 for any aim within a rifle/tank cone of +X, so the divide is safe.
    dir.y * Fixed::from_int(WALL_X) / dir.x
}

/// Mean of a slice of `Fixed` (for comparing where a whole burst's pattern sits on the wall).
fn mean(v: &[Fixed]) -> Fixed {
    let mut sum = Fixed::ZERO;
    for x in v {
        sum = sum + *x;
    }
    sum / Fixed::from_int(v.len() as i32)
}

fn on_wall(y: Fixed) -> bool {
    y.abs() <= Fixed::from_int(WALL_HALF)
}

#[test]
fn rifleman_empties_a_magazine_into_a_single_spot() {
    // Pinpoint hitscan: every one of the 30 rounds lands on the EXACT same point of the wall, and
    // no RNG is consumed (a mastered rifle shot is never robbed by a random bullet).
    let mut rng = Rng::new(0xA11);
    let before = rng.checksum_state();

    let first = impact_y(Angle(0), Fixed::ZERO, &mut rng);
    assert!(on_wall(first), "the aimed shot lands on the wall");
    for _ in 1..MAG {
        let y = impact_y(Angle(0), Fixed::ZERO, &mut rng);
        assert_eq!(y, first, "every rifle round lands on the identical spot (no spread)");
    }
    assert_eq!(rng.checksum_state(), before, "a pinpoint burst draws no RNG");
}

#[test]
fn panning_the_rifle_aim_walks_the_impact_across_the_wall() {
    // "Move his sight left / right and the rounds land on the left / right of the wall." With no
    // spread the point of impact tracks the aim monotonically.
    let mut rng = Rng::new(0);
    let left = impact_y(Angle(PAN), Fixed::ZERO, &mut rng); //  aim one way
    let centre = impact_y(Angle(0), Fixed::ZERO, &mut rng); //  dead ahead
    let right = impact_y(Angle(-PAN), Fixed::ZERO, &mut rng); // aim the other way

    assert!(left > centre && centre > right, "impact walks across the wall with the aim");
    assert!(centre.abs() <= Fixed::from_ratio(1, 100), "aimed dead-centre → hits dead-centre");
    assert!(on_wall(left) && on_wall(right), "the panned shots still land on the wall");
}

#[test]
fn tank_spray_scatters_across_the_wall_and_is_deterministic() {
    // A blown tank reticle sprays: the magazine lands across a *spread* of points — all on the wall,
    // actually varied (not a single dot) — and the pattern is bit-identical for the same seed
    // (lockstep, invariant #7).
    let mut a = Rng::new(0x5CA77E5);
    let mut b = Rng::new(0x5CA77E5);

    let mut pattern_a = Vec::new();
    for _ in 0..MAG {
        let ya = impact_y(Angle(0), DISPERSION_MAX, &mut a);
        let yb = impact_y(Angle(0), DISPERSION_MAX, &mut b);
        assert_eq!(ya, yb, "same seed → identical spray pattern (lockstep)");
        assert!(on_wall(ya), "every sprayed round still lands on the wall panel");
        pattern_a.push(ya);
    }

    let lo = pattern_a.iter().copied().fold(pattern_a[0], Fixed::min);
    let hi = pattern_a.iter().copied().fold(pattern_a[0], Fixed::max);
    assert!(hi > lo, "the tank spray actually scatters across the wall (not one spot)");
}

#[test]
fn tank_spray_recentres_when_the_gun_re_aims() {
    // Shift the aim and the whole scatter pattern moves with it — the spray on the left aim sits, on
    // average, further along the wall than the spray dead-ahead, which sits further than the right.
    let sample = |heading: Angle, seed: u64| -> Fixed {
        let mut rng = Rng::new(seed);
        let ys: Vec<Fixed> = (0..MAG).map(|_| impact_y(heading, DISPERSION_MAX, &mut rng)).collect();
        for y in &ys {
            assert!(on_wall(*y), "re-aimed spray stays on the wall");
        }
        mean(&ys)
    };

    // Same seed for each so the comparison isolates the aim shift, not RNG luck.
    let left = sample(Angle(PAN), 0xBEEF);
    let centre = sample(Angle(0), 0xBEEF);
    let right = sample(Angle(-PAN), 0xBEEF);

    assert!(left > centre && centre > right, "the scatter pattern re-centres on the new aim");
}

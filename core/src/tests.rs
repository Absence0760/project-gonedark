//! Core determinism + math tests. These run in CI on every target in the matrix
//! (docs/phase-1-plan.md §6); a cross-arch divergence shows up as a checksum mismatch.

use crate::checksum::Checksum;
use crate::components::{InputSource, Order, Stance, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::flow_field::FlowField;
use crate::rng::Rng;
use crate::sim::{Command, Sim};
use crate::trig::{self, Angle, ANGLE_FULL};

#[test]
fn fixed_arithmetic() {
    assert_eq!((Fixed::from_int(2) * Fixed::from_int(3)).to_int(), 6);
    assert_eq!(
        Fixed::from_int(7) / Fixed::from_int(2),
        Fixed::from_ratio(7, 2)
    );
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
    assert!(
        dir.x > Fixed::ZERO,
        "should steer +x toward goal, got {dir:?}"
    );
    assert!(
        dir.y > Fixed::ZERO,
        "should steer +y toward goal, got {dir:?}"
    );

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

// ---------------------------------------------------------------------------
// fixed — the Q16 dot 16 fixed-point scalar. A wrong result here desyncs downstream.
// ---------------------------------------------------------------------------

#[test]
fn fixed_to_int_truncates_toward_negative_infinity() {
    // to_int is an arithmetic shift, so it floors — not truncates toward zero.
    assert_eq!(Fixed::from_ratio(-1, 2).to_int(), -1);
    assert_eq!(Fixed::from_ratio(-3, 2).to_int(), -2);
    assert_eq!(Fixed::from_ratio(3, 2).to_int(), 1);
    assert_eq!(Fixed::from_ratio(1, 2).to_int(), 0);
    assert_eq!(Fixed::from_int(-5).to_int(), -5);
}

#[test]
fn fixed_from_ratio_is_exact_for_dyadic_fractions() {
    // Powers-of-two denominators are representable exactly in this fixed-point format.
    assert_eq!(Fixed::from_ratio(1, 2), Fixed::HALF);
    assert_eq!(
        Fixed::from_ratio(1, 4) + Fixed::from_ratio(1, 4),
        Fixed::HALF
    );
    assert_eq!(Fixed::from_ratio(3, 4).to_bits(), Fixed::SCALE * 3 / 4);
    assert_eq!(Fixed::from_ratio(-1, 4).to_bits(), -(Fixed::SCALE / 4));
}

#[test]
fn fixed_mul_div_round_trip_and_precision() {
    // (a / b) * b recovers a for dyadic values; mul uses an i64 intermediate so no overflow.
    let a = Fixed::from_int(7);
    let b = Fixed::from_int(2);
    assert_eq!(a / b, Fixed::from_ratio(7, 2));
    assert_eq!((a / b) * b, a);
    // Fractional multiply: 1/2 * 1/2 = 1/4, exact.
    assert_eq!(Fixed::HALF * Fixed::HALF, Fixed::from_ratio(1, 4));
    // A large-by-small product keeps precision through the i64 path.
    assert_eq!(
        Fixed::from_int(1000) * Fixed::from_ratio(1, 8),
        Fixed::from_ratio(1000, 8)
    );
}

#[test]
fn fixed_abs_signum_neg() {
    // abs/neg/signum on negative, zero, positive.
    assert_eq!(Fixed::from_int(-3).abs(), Fixed::from_int(3));
    assert_eq!(Fixed::from_int(3).abs(), Fixed::from_int(3));
    assert_eq!(Fixed::ZERO.abs(), Fixed::ZERO);
    assert_eq!(-Fixed::from_int(4), Fixed::from_int(-4));
    assert_eq!(-Fixed::ZERO, Fixed::ZERO);
    assert_eq!(Fixed::from_int(9).signum(), Fixed::ONE);
    assert_eq!(Fixed::from_int(-9).signum(), Fixed::from_int(-1));
    assert_eq!(Fixed::ZERO.signum(), Fixed::ZERO);
    // signum of a sub-unit fraction still reports direction, not magnitude.
    assert_eq!(Fixed::from_ratio(1, 1000).signum(), Fixed::ONE);
    assert_eq!(Fixed::from_ratio(-1, 1000).signum(), Fixed::from_int(-1));
}

#[test]
fn fixed_min_max() {
    // min/max pick by ordered value, including across zero and equal inputs.
    let a = Fixed::from_int(-2);
    let b = Fixed::from_int(5);
    assert_eq!(a.min(b), a);
    assert_eq!(a.max(b), b);
    assert_eq!(b.min(a), a);
    assert_eq!(b.max(a), b);
    assert_eq!(a.min(a), a);
    assert_eq!(a.max(a), a);
}

#[test]
fn fixed_from_bits_to_bits_round_trip() {
    // Raw bit reinterpretation is the renderer's only entry point; it must round-trip.
    for &bits in &[
        0_i32,
        1,
        -1,
        Fixed::SCALE,
        -Fixed::SCALE,
        i32::MAX,
        i32::MIN,
        123_456,
    ] {
        assert_eq!(Fixed::from_bits(bits).to_bits(), bits);
    }
    // ONE/HALF expose the expected raw layout.
    assert_eq!(Fixed::ONE.to_bits(), Fixed::SCALE);
    assert_eq!(Fixed::HALF.to_bits(), Fixed::SCALE / 2);
    assert_eq!(Fixed::ZERO.to_bits(), 0);
}

#[test]
fn fixed_wrapping_add_sub_at_extremes() {
    // Add/sub are explicitly wrapping so debug/release/arch all agree at the i32 edges.
    assert_eq!(Fixed::MAX + Fixed::from_bits(1), Fixed::MIN);
    assert_eq!(Fixed::MIN - Fixed::from_bits(1), Fixed::MAX);
    assert_eq!(
        Fixed::MAX.wrapping_add(Fixed::ONE).to_bits(),
        i32::MAX.wrapping_add(Fixed::SCALE)
    );
    assert_eq!(
        Fixed::MIN.wrapping_sub(Fixed::ONE).to_bits(),
        i32::MIN.wrapping_sub(Fixed::SCALE)
    );
}

#[test]
fn fixed_assign_ops_match_value_ops() {
    // AddAssign/SubAssign must agree bit-for-bit with the value forms.
    let mut a = Fixed::from_int(3);
    a += Fixed::from_ratio(1, 2);
    assert_eq!(a, Fixed::from_ratio(7, 2));
    a -= Fixed::from_int(1);
    assert_eq!(a, Fixed::from_ratio(5, 2));
}

#[test]
fn fixed_ordering_is_total_and_signed() {
    // Ord derives from the i32 bits; assert the sign ordering the sim relies on.
    assert!(Fixed::from_int(-1) < Fixed::ZERO);
    assert!(Fixed::ZERO < Fixed::ONE);
    assert!(Fixed::HALF < Fixed::ONE);
    assert!(Fixed::MIN < Fixed::MAX);
}

// ---------------------------------------------------------------------------
// trig — LUT sin/cos + integer sqrt. Drift here corrupts movement.
// ---------------------------------------------------------------------------

#[test]
fn sin_cos_quarter_landmarks() {
    // The four cardinal turns, within one LUT-step tolerance.
    let tol = Fixed::from_ratio(1, 1000);
    assert_eq!(trig::sin(Angle(0)), Fixed::ZERO);
    assert!((trig::sin(Angle(ANGLE_FULL / 4)) - Fixed::ONE).abs() <= tol);
    assert!(trig::sin(Angle(ANGLE_FULL / 2)).abs() <= tol);
    assert!((trig::sin(Angle(3 * ANGLE_FULL / 4)) + Fixed::ONE).abs() <= tol);
    assert!((trig::cos(Angle(0)) - Fixed::ONE).abs() <= tol);
    assert!(trig::cos(Angle(ANGLE_FULL / 4)).abs() <= tol);
    assert!((trig::cos(Angle(ANGLE_FULL / 2)) + Fixed::ONE).abs() <= tol);
    assert!(trig::cos(Angle(3 * ANGLE_FULL / 4)).abs() <= tol);
}

#[test]
fn cos_equals_sin_shifted_a_quarter_turn() {
    // cos(a) == sin(a + quarter) by construction — exact (same LUT index), every quadrant.
    for &a in &[
        0,
        1234,
        ANGLE_FULL / 8,
        ANGLE_FULL / 3,
        3 * ANGLE_FULL / 4,
        50_000,
    ] {
        assert_eq!(trig::cos(Angle(a)), trig::sin(Angle(a + ANGLE_FULL / 4)));
    }
}

#[test]
fn angle_wrap_masks_into_one_turn() {
    // wrap() reduces any angle into [0, ANGLE_FULL) and is periodic by a full turn.
    assert_eq!(Angle(0).wrap(), 0);
    assert_eq!(Angle(ANGLE_FULL).wrap(), 0);
    assert_eq!(Angle(ANGLE_FULL + 7).wrap(), 7);
    assert_eq!(Angle(-1).wrap(), ANGLE_FULL - 1);
    assert_eq!(Angle(ANGLE_FULL / 4).wrap(), ANGLE_FULL / 4);
    // sin is therefore periodic across the wrap.
    assert_eq!(trig::sin(Angle(123)), trig::sin(Angle(123 + ANGLE_FULL)));
    assert_eq!(trig::sin(Angle(123)), trig::sin(Angle(123 - ANGLE_FULL)));
}

#[test]
fn sqrt_non_perfect_squares_are_bounded() {
    // For r = sqrt(x): r*r <= x < (r+1)*(r+1), all in Fixed — the integer-isqrt contract.
    for n in [2, 3, 5, 7, 10, 50, 99, 1000] {
        let x = Fixed::from_int(n);
        let r = trig::sqrt(x);
        let next = r + Fixed::ONE;
        assert!(r * r <= x, "sqrt({n}) overshoots: {r:?}");
        assert!(x < next * next, "sqrt({n}) undershoots: {r:?}");
    }
}

#[test]
fn sqrt_zero_and_negative_are_zero() {
    // sqrt(0)==0 and any negative input clamps to 0 (never panics, never NaN-equivalent).
    assert_eq!(trig::sqrt(Fixed::ZERO), Fixed::ZERO);
    assert_eq!(trig::sqrt(Fixed::from_int(-1)), Fixed::ZERO);
    assert_eq!(trig::sqrt(Fixed::MIN), Fixed::ZERO);
}

#[test]
fn sqrt_of_a_fraction_is_bounded() {
    // sqrt(1/4) == 1/2 exactly. For sqrt(1/2), check the exact integer-isqrt contract in raw
    // bits (Fixed multiply truncates, so assert on the u64 radicand the impl actually uses):
    // r*r <= (bits << 16) < (r+1)*(r+1).
    assert_eq!(trig::sqrt(Fixed::from_ratio(1, 4)), Fixed::HALF);
    let x = Fixed::HALF;
    let r = trig::sqrt(x).to_bits() as u64;
    let radicand = (x.to_bits() as u64) << Fixed::FRAC_BITS;
    assert!(r * r <= radicand, "sqrt(1/2) overshoots");
    assert!(radicand < (r + 1) * (r + 1), "sqrt(1/2) undershoots");
}

// ---------------------------------------------------------------------------
// rng — PCG32. Determinism is the whole point; a drift here desyncs lockstep.
// ---------------------------------------------------------------------------

#[test]
fn rng_same_seed_same_stream() {
    // Identical seed + identical call sequence ⇒ bit-identical stream.
    let mut a = Rng::new(0xDEADBEEF);
    let mut b = Rng::new(0xDEADBEEF);
    for _ in 0..256 {
        assert_eq!(a.next_u32(), b.next_u32());
    }
}

#[test]
fn rng_different_seeds_diverge() {
    // Different seeds must produce different streams. Close seeds can collide on the *first*
    // draw (PCG shares one stream selector), so compare a window — they must differ somewhere.
    let mut a = Rng::new(1);
    let mut b = Rng::new(2);
    let stream_a: Vec<u32> = (0..16).map(|_| a.next_u32()).collect();
    let stream_b: Vec<u32> = (0..16).map(|_| b.next_u32()).collect();
    assert_ne!(stream_a, stream_b);
}

#[test]
fn rng_next_u64_is_two_u32_draws() {
    // next_u64 is defined as (hi << 32) | lo over two next_u32 calls — assert that exactly.
    let mut a = Rng::new(42);
    let mut b = Rng::new(42);
    let hi = b.next_u32() as u64;
    let lo = b.next_u32() as u64;
    assert_eq!(a.next_u64(), (hi << 32) | lo);
}

#[test]
fn rng_below_is_in_range() {
    // below(n) is always < n over a long stream, including the degenerate below(1)==0.
    let mut r = Rng::new(0xABCDEF);
    for _ in 0..10_000 {
        assert!(r.below(7) < 7);
        assert!(r.below(1) == 0);
        assert!(r.below(100) < 100);
    }
}

#[test]
fn rng_below_covers_its_range() {
    // Over many draws, below(n) should hit every bucket — guards against a stuck generator.
    let mut r = Rng::new(123_456);
    let mut seen = [false; 6];
    for _ in 0..10_000 {
        seen[r.below(6) as usize] = true;
    }
    assert!(
        seen.iter().all(|&s| s),
        "below(6) failed to cover all buckets"
    );
}

// ---------------------------------------------------------------------------
// checksum — FNV-1a desync detector. Must be deterministic and order-sensitive.
// ---------------------------------------------------------------------------

#[test]
fn checksum_deterministic_for_identical_input() {
    // Same bytes in same order ⇒ same hash, every run.
    let mut a = Checksum::new();
    let mut b = Checksum::new();
    for v in [1_i32, -2, 3, 4] {
        a.write_i32(v);
        b.write_i32(v);
    }
    assert_eq!(a.finish(), b.finish());
}

#[test]
fn checksum_is_order_sensitive() {
    // Writing a,b must differ from b,a — order is part of the state fold.
    let mut ab = Checksum::new();
    ab.write_u8(0xAA);
    ab.write_u8(0xBB);
    let mut ba = Checksum::new();
    ba.write_u8(0xBB);
    ba.write_u8(0xAA);
    assert_ne!(ab.finish(), ba.finish());
}

#[test]
fn checksum_different_bytes_differ() {
    // A single differing byte changes the hash.
    let mut a = Checksum::new();
    a.write_u32(0x1000_0000);
    let mut b = Checksum::new();
    b.write_u32(0x1000_0001);
    assert_ne!(a.finish(), b.finish());
}

#[test]
fn checksum_empty_is_the_fnv_offset_and_default_matches_new() {
    // An untouched hasher reports the FNV offset basis; Default == new().
    assert_eq!(Checksum::new().finish(), 0xcbf2_9ce4_8422_2325);
    assert_eq!(Checksum::default().finish(), Checksum::new().finish());
}

#[test]
fn checksum_width_helpers_agree_with_byte_writes() {
    // write_u64 must equal eight little-endian write_u8 calls (endianness-stable contract).
    let v: u64 = 0x0102_0304_0506_0708;
    let mut wide = Checksum::new();
    wide.write_u64(v);
    let mut bytes = Checksum::new();
    for b in v.to_le_bytes() {
        bytes.write_u8(b);
    }
    assert_eq!(wide.finish(), bytes.finish());
}

// ---------------------------------------------------------------------------
// ecs — generational SoA store. Slot recycling + stale-handle detection.
// ---------------------------------------------------------------------------

#[test]
fn ecs_spawn_starts_at_generation_zero() {
    // A fresh slot is generation 0 and live; capacity tracks slot count.
    let mut w = World::new();
    let e = w.spawn();
    assert_eq!(e.generation, 0);
    assert_eq!(e.index, 0);
    assert!(w.is_alive(e));
    assert_eq!(w.capacity(), 1);
}

#[test]
fn ecs_despawn_then_spawn_recycles_slot_with_bumped_generation() {
    // The free list is a stack: respawn reuses the same index but bumps generation.
    let mut w = World::new();
    let a = w.spawn();
    w.despawn(a);
    assert!(!w.is_alive(a));
    let b = w.spawn();
    assert_eq!(b.index, a.index, "slot should be recycled");
    assert_eq!(b.generation, a.generation + 1, "generation should bump");
    assert!(w.is_alive(b));
    // No new slot was allocated for the recycle.
    assert_eq!(w.capacity(), 1);
}

#[test]
fn ecs_stale_handle_to_recycled_slot_is_not_alive() {
    // The old handle into a recycled slot must read dead even though the slot is now live.
    let mut w = World::new();
    let stale = w.spawn();
    w.despawn(stale);
    let fresh = w.spawn();
    assert!(!w.is_alive(stale));
    assert!(w.is_alive(fresh));
    assert_ne!(stale.generation, fresh.generation);
}

#[test]
fn ecs_respawn_resets_component_arrays() {
    // A recycled slot must come back zeroed — leftover state would desync the sim.
    let mut w = World::new();
    let a = w.spawn();
    let i = a.index as usize;
    w.pos[i] = Vec2::new(Fixed::from_int(9), Fixed::from_int(-4));
    w.vel[i] = Vec2::new(Fixed::ONE, Fixed::ONE);
    w.order[i] = Order::MoveTo(Vec2::new(Fixed::from_int(1), Fixed::ZERO));
    w.stance[i] = Stance::FireAtWill;
    w.input_source[i] = InputSource::Embodied;
    w.despawn(a);
    let b = w.spawn();
    let j = b.index as usize;
    assert_eq!(w.pos[j], Vec2::ZERO);
    assert_eq!(w.vel[j], Vec2::ZERO);
    assert_eq!(w.order[j], Order::Idle);
    assert_eq!(w.stance[j], Stance::ReturnFire);
    assert_eq!(w.input_source[j], InputSource::Orders);
}

#[test]
fn ecs_double_despawn_is_a_no_op() {
    // Despawning a stale handle twice must not bump generation again or corrupt the free list.
    let mut w = World::new();
    let a = w.spawn();
    w.despawn(a);
    w.despawn(a); // stale: must be ignored
    let b = w.spawn();
    assert_eq!(b.generation, 1, "second despawn must not have bumped again");
    // The free list isn't double-stacked: a second spawn allocates a brand-new slot.
    let c = w.spawn();
    assert_eq!(c.index, 1);
    assert_eq!(w.capacity(), 2);
}

#[test]
fn ecs_spawn_despawn_spawn_order_is_deterministic() {
    // The exact index/generation sequence must be reproducible across runs (lockstep).
    fn run() -> Vec<(u32, u32)> {
        let mut w = World::new();
        let mut out = Vec::new();
        let a = w.spawn();
        let b = w.spawn();
        let c = w.spawn();
        out.push((a.index, a.generation));
        out.push((b.index, b.generation));
        out.push((c.index, c.generation));
        w.despawn(b);
        w.despawn(a);
        let d = w.spawn(); // pops a (last freed) → index 0
        let e = w.spawn(); // pops b → index 1
        out.push((d.index, d.generation));
        out.push((e.index, e.generation));
        let _ = c;
        out
    }
    let first = run();
    let second = run();
    assert_eq!(first, second);
    // Free list is LIFO: last despawned (a, index 0) is reused first.
    assert_eq!(first[3], (0, 1));
    assert_eq!(first[4], (1, 1));
}

// ---------------------------------------------------------------------------
// components — Vec2 fixed-point math + component defaults.
// ---------------------------------------------------------------------------

#[test]
fn vec2_add_sub_scale_dot_len_sq() {
    // Core vector arithmetic, all exact in fixed-point.
    let a = Vec2::new(Fixed::from_int(3), Fixed::from_int(4));
    let b = Vec2::new(Fixed::from_int(1), Fixed::from_int(2));
    assert_eq!(a + b, Vec2::new(Fixed::from_int(4), Fixed::from_int(6)));
    assert_eq!(a - b, Vec2::new(Fixed::from_int(2), Fixed::from_int(2)));
    assert_eq!(
        a.scale(Fixed::from_int(2)),
        Vec2::new(Fixed::from_int(6), Fixed::from_int(8))
    );
    assert_eq!(a.dot(b), Fixed::from_int(11)); // 3*1 + 4*2
    assert_eq!(a.len_sq(), Fixed::from_int(25)); // 9 + 16
}

#[test]
fn vec2_len_of_3_4_is_5() {
    // The textbook 3-4-5 triangle: len is exact for this perfect-square sum.
    let v = Vec2::new(Fixed::from_int(3), Fixed::from_int(4));
    assert_eq!(v.len(), Fixed::from_int(5));
}

#[test]
fn vec2_zero_len_and_normalized_are_safe() {
    // A zero vector must not divide by zero: len==0 and normalized==zero.
    assert_eq!(Vec2::ZERO.len(), Fixed::ZERO);
    assert_eq!(Vec2::ZERO.normalized(), Vec2::ZERO);
}

#[test]
fn vec2_normalized_axis_is_unit() {
    // An axis-aligned vector normalizes to the corresponding unit basis vector.
    let along_x = Vec2::new(Fixed::from_int(7), Fixed::ZERO);
    assert_eq!(along_x.normalized(), Vec2::new(Fixed::ONE, Fixed::ZERO));
    let neg_y = Vec2::new(Fixed::ZERO, Fixed::from_int(-3));
    assert_eq!(
        neg_y.normalized(),
        Vec2::new(Fixed::ZERO, Fixed::from_int(-1))
    );
}

#[test]
fn vec2_normalized_is_bounded_unit_length() {
    // A general normalized vector has length within one Fixed step of 1.
    let v = Vec2::new(Fixed::from_int(5), Fixed::from_int(12)); // len 13
    let n = v.normalized();
    let len_sq = n.len_sq();
    let lo = Fixed::from_ratio(63, 64);
    let hi = Fixed::from_ratio(65, 64);
    assert!(
        len_sq >= lo && len_sq <= hi,
        "normalized len_sq off unit: {len_sq:?}"
    );
}

#[test]
fn component_defaults() {
    // The literal-executor defaults: Idle order, ReturnFire stance, Orders input.
    assert_eq!(Order::default(), Order::Idle);
    assert_eq!(Stance::default(), Stance::ReturnFire);
    assert_eq!(InputSource::default(), InputSource::Orders);
    assert_eq!(Vec2::default(), Vec2::ZERO);
}

// ---------------------------------------------------------------------------
// sim — command application, checksum stability, snapshot fidelity, ticks.
// ---------------------------------------------------------------------------

#[test]
fn sim_move_command_sets_move_order() {
    // A Move command must install Order::MoveTo(target) on a live unit.
    let mut sim = Sim::new(1);
    let e = sim.world.spawn();
    let target = Vec2::new(Fixed::from_int(4), Fixed::from_int(9));
    sim.step(&[Command::Move { entity: e, target }]);
    // After one tick the unit has begun moving, but its order is MoveTo until it arrives.
    assert_eq!(sim.world.order[e.index as usize], Order::MoveTo(target));
}

#[test]
fn sim_set_stance_command_applies() {
    // SetStance must overwrite the stance component.
    let mut sim = Sim::new(1);
    let e = sim.world.spawn();
    sim.step(&[Command::SetStance {
        entity: e,
        stance: Stance::HoldFire,
    }]);
    assert_eq!(sim.world.stance[e.index as usize], Stance::HoldFire);
    sim.step(&[Command::SetStance {
        entity: e,
        stance: Stance::FireAtWill,
    }]);
    assert_eq!(sim.world.stance[e.index as usize], Stance::FireAtWill);
}

#[test]
fn sim_embody_and_surface_flip_input_source() {
    // Possession is purely an input-source swap (invariant #5) — assert both directions.
    let mut sim = Sim::new(1);
    let e = sim.world.spawn();
    assert_eq!(
        sim.world.input_source[e.index as usize],
        InputSource::Orders
    );
    sim.step(&[Command::Embody { entity: e }]);
    assert_eq!(
        sim.world.input_source[e.index as usize],
        InputSource::Embodied
    );
    sim.step(&[Command::Surface { entity: e }]);
    assert_eq!(
        sim.world.input_source[e.index as usize],
        InputSource::Orders
    );
}

#[test]
fn sim_commands_to_dead_entity_are_no_ops() {
    // A command targeting a despawned (or stale) entity must change nothing.
    let mut sim = Sim::new(1);
    let e = sim.world.spawn();
    sim.world.despawn(e);
    let before = sim.checksum();
    sim.step(&[
        Command::Move {
            entity: e,
            target: Vec2::new(Fixed::from_int(5), Fixed::ZERO),
        },
        Command::SetStance {
            entity: e,
            stance: Stance::FireAtWill,
        },
        Command::Embody { entity: e },
        Command::Surface { entity: e },
    ]);
    // Only the tick counter advanced; the dead slot's components are untouched.
    assert_eq!(sim.world.order[e.index as usize], Order::Idle);
    assert_eq!(sim.world.stance[e.index as usize], Stance::ReturnFire);
    assert_eq!(
        sim.world.input_source[e.index as usize],
        InputSource::Orders
    );
    // The checksum still moved because tick advanced — but only because of the tick.
    assert_ne!(before, sim.checksum());
}

#[test]
fn sim_tick_count_increments_per_step() {
    // tick_count is exactly the number of step() calls.
    let mut sim = Sim::new(7);
    assert_eq!(sim.tick_count(), 0);
    for n in 1..=5 {
        sim.step(&[]);
        assert_eq!(sim.tick_count(), n);
    }
}

#[test]
fn sim_checksum_changes_after_state_changing_tick() {
    // Moving a unit changes the folded state, so the checksum must change between ticks.
    let mut sim = Sim::new(0x1234);
    let e = sim.world.spawn();
    sim.step(&[Command::Move {
        entity: e,
        target: Vec2::new(Fixed::from_int(20), Fixed::ZERO),
    }]);
    let c1 = sim.checksum();
    sim.step(&[]);
    let c2 = sim.checksum();
    assert_ne!(c1, c2, "a tick that moves a unit must change the checksum");
}

#[test]
fn sim_checksum_is_stable_for_identical_runs() {
    // Two independent sims fed the identical script agree on the checksum every tick.
    let mut a = Sim::new(0x55AA);
    let mut b = Sim::new(0x55AA);
    let ea = a.world.spawn();
    let eb = b.world.spawn();
    let target = Vec2::new(Fixed::from_int(-7), Fixed::from_int(11));
    a.step(&[Command::Move { entity: ea, target }]);
    b.step(&[Command::Move { entity: eb, target }]);
    for _ in 0..50 {
        a.step(&[]);
        b.step(&[]);
        assert_eq!(a.checksum(), b.checksum());
    }
}

#[test]
fn snapshot_reflects_live_units_and_embodied_flag() {
    // The render snapshot carries live units with the right embodied flag and tick.
    let mut sim = Sim::new(9);
    let a = sim.world.spawn();
    let b = sim.world.spawn();
    sim.step(&[Command::Embody { entity: a }]);
    let snap = sim.snapshot();
    assert_eq!(snap.tick, sim.tick_count());
    assert_eq!(snap.units.len(), 2);
    assert!(snap.units[0].embodied, "first unit was embodied");
    assert!(!snap.units[1].embodied, "second unit is order-driven");
    let _ = b;
}

#[test]
fn snapshot_skips_dead_units() {
    // Despawned units must not appear in the render snapshot.
    let mut sim = Sim::new(9);
    let a = sim.world.spawn();
    let b = sim.world.spawn();
    let c = sim.world.spawn();
    sim.world.despawn(b);
    let snap = sim.snapshot();
    assert_eq!(snap.units.len(), 2, "only the two live units should appear");
    let _ = (a, c);
}

#[test]
fn embody_stops_motion_surface_resumes_it() {
    // Embodying freezes order-driven motion; surfacing hands the unit back to its order.
    let mut sim = Sim::new(0xBEEF);
    let e = sim.world.spawn();
    let target = Vec2::new(Fixed::from_int(40), Fixed::ZERO);
    sim.step(&[
        Command::Move { entity: e, target },
        Command::Embody { entity: e },
    ]);
    let frozen = sim.world.pos[e.index as usize];
    for _ in 0..20 {
        sim.step(&[]);
    }
    assert_eq!(
        sim.world.pos[e.index as usize], frozen,
        "embodied unit must not drift"
    );
    // Surface and let the literal executor resume toward the same target.
    sim.step(&[Command::Surface { entity: e }]);
    for _ in 0..400 {
        sim.step(&[]);
    }
    assert!(
        (sim.world.pos[e.index as usize] - target).len_sq() <= Fixed::from_ratio(1, 16),
        "surfaced unit should resume and reach its order target"
    );
}

#[test]
fn idle_unit_has_zero_velocity_and_stays_put() {
    // A unit with no order holds position and reports zero velocity every tick.
    let mut sim = Sim::new(3);
    let e = sim.world.spawn();
    let start = sim.world.pos[e.index as usize];
    for _ in 0..30 {
        sim.step(&[]);
    }
    assert_eq!(sim.world.pos[e.index as usize], start);
    assert_eq!(sim.world.vel[e.index as usize], Vec2::ZERO);
}

//! Core determinism + math tests. These run in CI on every target in the matrix
//! (docs/phase-1-plan.md §6); a cross-arch divergence shows up as a checksum mismatch.

use crate::checksum::Checksum;
use crate::components::{InputSource, Order, Stance, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::flow_field::{FlowField, FlowFieldCache};
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
fn flow_field_cache_matches_fresh_build() {
    // The Phase 3 cache must return a field bit-identical (in everything observable) to a fresh
    // FlowField::build — that equivalence is what makes the optimisation determinism-safe.
    let goals = [
        Vec2::new(Fixed::from_int(40), Fixed::ZERO),
        Vec2::new(Fixed::from_int(-40), Fixed::ZERO),
        Vec2::new(Fixed::from_int(12), Fixed::from_int(-7)),
        Vec2::ZERO,
    ];
    let probes = [
        Vec2::ZERO,
        Vec2::new(Fixed::from_int(-30), Fixed::from_int(20)),
        Vec2::new(Fixed::from_int(40), Fixed::from_int(40)),
        Vec2::new(Fixed::from_int(-50), Fixed::from_int(-50)),
        Vec2::new(Fixed::from_int(9000), Fixed::from_int(-9000)),
    ];
    let mut cache = FlowFieldCache::new();
    for g in goals {
        let fresh = FlowField::build(g);
        let cached = cache.get(g);
        for p in probes {
            assert_eq!(
                cached.sample(p),
                fresh.sample(p),
                "sample mismatch for goal {g:?}"
            );
            assert_eq!(
                cached.cost_at(p),
                fresh.cost_at(p),
                "cost mismatch for goal {g:?}"
            );
        }
    }

    // Hit path: a repeated request for an already-built goal must return a field that still
    // samples bit-identically to a fresh build (this is what units sharing a goal rely on).
    let g = goals[0];
    let fresh = FlowField::build(g);
    let cached = cache.get(g);
    for p in probes {
        assert_eq!(
            cached.sample(p),
            fresh.sample(p),
            "hit-path sample mismatch"
        );
        assert_eq!(
            cached.cost_at(p),
            fresh.cost_at(p),
            "hit-path cost mismatch"
        );
    }
}

#[test]
fn flow_field_cache_dedups_shared_goals() {
    // Repeated requests for the same goal build once; distinct goals each build. This is the
    // dedup that turns ~200 per-unit builds into a handful for a shared objective.
    let mut cache = FlowFieldCache::new();
    let g1 = Vec2::new(Fixed::from_int(40), Fixed::ZERO);
    let g2 = Vec2::new(Fixed::from_int(-40), Fixed::ZERO);
    let _ = cache.get(g1).cost_at(Vec2::ZERO);
    let _ = cache.get(g1).cost_at(Vec2::ZERO);
    let _ = cache.get(g2).cost_at(Vec2::ZERO);
    let _ = cache.get(g1).cost_at(Vec2::ZERO);
    assert_eq!(
        cache.distinct_goals(),
        2,
        "two distinct goals should build exactly two fields"
    );
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

#[test]
fn set_order_and_retreat_threshold_commands_apply() {
    // The richer order vocabulary is reachable through `Command` (the touch UI in `engine`
    // emits these): `SetOrder` installs an arbitrary Phase-2 order, `SetRetreatThreshold`
    // pre-programs the fall-back trigger. Both are folded into the per-tick checksum.
    use crate::components::Health;
    let mut sim = Sim::new(7);
    let e = sim.world.spawn();
    let i = e.index as usize;
    // Full HP so combat never despawns it and the retreat trigger never fires (health > 30%).
    sim.world.health[i] = Health::full(Fixed::from_int(100));

    // HoldPosition is stable under the literal executor, so it survives the step intact.
    sim.step(&[Command::SetOrder {
        entity: e,
        order: Order::HoldPosition,
    }]);
    assert_eq!(sim.world.order[i], Order::HoldPosition);

    // The retreat fraction is stored verbatim (systems only consult it), so it survives a step.
    let frac = Fixed::from_ratio(3, 10);
    sim.step(&[Command::SetRetreatThreshold {
        entity: e,
        fraction: frac,
    }]);
    assert_eq!(sim.world.retreat_below[i], frac);
    assert_eq!(
        sim.world.order[i],
        Order::HoldPosition,
        "healthy unit does not fall back"
    );
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
    // A recycled slot must come back zeroed — leftover state would desync the sim. Covers the
    // Phase 1 fields AND every Phase 2 field, so a future `spawn()` that forgets to reset one
    // (silent stale-state desync on slot recycle) is caught here.
    use crate::components::{
        Building, EntityKind, Faction, Health, ProductionItem, UnitKind, Weapon,
    };
    let mut w = World::new();
    let a = w.spawn();
    let i = a.index as usize;
    w.pos[i] = Vec2::new(Fixed::from_int(9), Fixed::from_int(-4));
    w.vel[i] = Vec2::new(Fixed::ONE, Fixed::ONE);
    w.order[i] = Order::MoveTo(Vec2::new(Fixed::from_int(1), Fixed::ZERO));
    w.stance[i] = Stance::FireAtWill;
    w.input_source[i] = InputSource::Embodied;
    w.faction[i] = Faction::Enemy;
    w.kind[i] = EntityKind::Building;
    w.health[i] = Health {
        cur: Fixed::from_int(3),
        max: Fixed::from_int(9),
    };
    w.weapon[i] = Weapon {
        range: Fixed::from_int(5),
        damage: Fixed::from_int(2),
        cooldown_ticks: 7,
        cooldown_left: 4,
    };
    w.suppression[i] = Fixed::HALF;
    w.last_attacker[i] = Some(a);
    w.retreat_below[i] = Fixed::from_ratio(1, 3);
    w.vision[i] = Fixed::from_int(99);
    w.building[i] = Building {
        kind: crate::components::BuildingKind::Camp,
        level: 5,
        build_ticks_left: 12,
        queue: vec![ProductionItem {
            kind: UnitKind::Heavy,
            ticks_left: 3,
        }],
    };
    w.despawn(a);
    let b = w.spawn();
    let j = b.index as usize;
    assert_eq!(w.pos[j], Vec2::ZERO);
    assert_eq!(w.vel[j], Vec2::ZERO);
    assert_eq!(w.order[j], Order::Idle);
    assert_eq!(w.stance[j], Stance::ReturnFire);
    assert_eq!(w.input_source[j], InputSource::Orders);
    // Phase 2 fields must reset to their defaults too.
    assert_eq!(w.faction[j], Faction::Player);
    assert_eq!(w.kind[j], EntityKind::Unit);
    assert_eq!(w.health[j], Health::default());
    assert_eq!(w.weapon[j], Weapon::default());
    assert_eq!(w.suppression[j], Fixed::ZERO);
    assert_eq!(w.last_attacker[j], None);
    assert_eq!(w.retreat_below[j], Fixed::ZERO);
    assert_eq!(w.vision[j], Fixed::from_int(24)); // DEFAULT_VISION
    assert_eq!(w.building[j], Building::default());
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

// ===========================================================================
// Authoritative snapshot serialize/deserialize (D28, Phase 3 workstream C).
//
// The load-bearing guard: serialize@T → deserialize → replay cmds[T..L] yields a checksum
// stream bit-identical to the never-interrupted run. Because these live here, they ride the
// determinism arch matrix automatically (invariant #7).
// ===========================================================================

use crate::components::{BuildingKind, EntityKind, Faction, UnitKind};
use crate::economy::{self, Resources};
use crate::ecs::Entity;
use crate::persist::{DeserializeError, Reader, StateSink, Writer};
use crate::territory::ControlPoint;

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

/// Spawn an armed rifleman of `faction` at `(x, y)` set to fire at will, returning its handle.
fn spawn_rifleman(sim: &mut Sim, x: i32, y: i32, faction: Faction) -> Entity {
    let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
    let ent = sim.world.spawn();
    let i = ent.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = v(x, y);
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = Stance::FireAtWill;
    ent
}

/// A non-trivial deterministic scene: two armed squads in firing range (so combat draws RNG and
/// despawns the dead — churning the free-list and bumping generations), a neutral control point,
/// stocked resources, and a player camp. Spawn order is fixed, so the handles are identical
/// across every sim built this way. Returns the handles a script drives.
struct Handles {
    p: [Entity; 3],
    e: [Entity; 3],
    camp: Entity,
}

fn battle_scene(sim: &mut Sim) -> Handles {
    sim.resources = Resources::new(100_000);
    sim.territory.points.push(ControlPoint::neutral(Vec2::ZERO));
    let p = [
        spawn_rifleman(sim, -5, 0, Faction::Player),
        spawn_rifleman(sim, -5, 3, Faction::Player),
        spawn_rifleman(sim, -6, 1, Faction::Player),
    ];
    let e = [
        spawn_rifleman(sim, 5, 0, Faction::Enemy),
        spawn_rifleman(sim, 5, 3, Faction::Enemy),
        spawn_rifleman(sim, 6, 1, Faction::Enemy),
    ];
    let camp = economy::build(
        &mut sim.world,
        &mut sim.resources,
        Faction::Player,
        BuildingKind::Camp,
        v(-20, 20),
    )
    .expect("camp affordable");
    Handles { p, e, camp }
}

/// A per-tick command script exercising a spread of the vocabulary at two ticks; quiet otherwise.
/// Drives both squads toward each other (so they engage and die), reshapes orders/stances, and
/// queues production at the camp — building a rich, churning world to snapshot.
fn script(h: &Handles, t: u64) -> Vec<Command> {
    match t {
        1 => vec![
            Command::AttackMove {
                entity: h.p[0],
                target: v(5, 0),
            },
            Command::AttackMove {
                entity: h.p[1],
                target: v(5, 3),
            },
            Command::SetOrder {
                entity: h.p[2],
                order: Order::Patrol {
                    a: v(-5, 1),
                    b: v(-5, -6),
                    toward_b: true,
                },
            },
            Command::AttackMove {
                entity: h.e[0],
                target: v(-5, 0),
            },
            Command::AttackMove {
                entity: h.e[1],
                target: v(-5, 3),
            },
            Command::SetRetreatThreshold {
                entity: h.e[2],
                fraction: Fixed::from_ratio(1, 3),
            },
            Command::QueueProduction {
                camp: h.camp,
                unit: UnitKind::Rifleman,
            },
        ],
        40 => vec![
            Command::Upgrade { camp: h.camp },
            Command::QueueProduction {
                camp: h.camp,
                unit: UnitKind::Heavy,
            },
        ],
        _ => Vec::new(),
    }
}

const SNAP_SEED: u64 = 0x9E3779B97F4A7C15;

/// THE headline guard: serialize@T → deserialize → replay must reproduce the never-interrupted
/// checksum stream exactly. Run for several T spread across the battle (early, mid-fight, late),
/// each time deserializing a fresh sim and replaying the remaining commands through plain `step`.
#[test]
fn snapshot_resume_matches_uninterrupted_run() {
    const L: u64 = 120;

    // Reference: one sim run straight through, recording every per-tick checksum.
    let mut refsim = Sim::new(SNAP_SEED);
    let h = battle_scene(&mut refsim);
    let mut refsums = Vec::with_capacity(L as usize);
    for t in 0..L {
        refsim.step(&script(&h, t));
        refsums.push(refsim.checksum());
    }

    for &cut in &[1u64, 5, 30, 41, 80, 119] {
        // Run a separate sim up to `cut`, snapshot it, then resume a NEW sim from the bytes.
        let mut sim = Sim::new(SNAP_SEED);
        let h2 = battle_scene(&mut sim);
        for t in 0..cut {
            sim.step(&script(&h2, t));
        }
        let bytes = sim.serialize();
        let mut resumed = Sim::deserialize(&bytes).expect("snapshot must deserialize");

        // The resumed sim must already agree at the cut boundary (checksum of the snapshotted
        // state == the reference's checksum at the same tick).
        assert_eq!(
            resumed.checksum(),
            sim.checksum(),
            "deserialized state diverges from the snapshotted sim at cut {cut}"
        );

        // Replay the remaining commands through a plain step loop; every tick's checksum must
        // match the never-interrupted run — bit for bit.
        for t in cut..L {
            resumed.step(&script(&h2, t));
            assert_eq!(
                resumed.checksum(),
                refsums[t as usize],
                "resume@{cut}: checksum diverged at tick {t}"
            );
        }
    }
}

/// `deserialize(serialize(sim))` reproduces an identical checksum AND serializes to the same
/// bytes — a complete, stable round-trip over a churned (deaths, free-list, production) world.
#[test]
fn snapshot_round_trip_is_byte_and_checksum_stable() {
    let mut sim = Sim::new(SNAP_SEED);
    let h = battle_scene(&mut sim);
    for t in 0..75 {
        sim.step(&script(&h, t));
    }
    // Explicitly churn the free-list and generations (despawn two units), then step on so the
    // freed slots may be recycled by production — exercising the resume-only liveness extras
    // (generation[] + free-list order) the round-trip must preserve, independent of combat
    // balance/timing.
    sim.world.despawn(h.p[1]);
    sim.world.despawn(h.e[2]);
    for t in 75..90 {
        sim.step(&script(&h, t));
    }
    assert!(
        !sim.world.free_list().is_empty() || sim.world.generations().iter().any(|&g| g > 0),
        "scene should have churned the liveness triple (free-list or a bumped generation)"
    );

    let bytes = sim.serialize();
    let restored = Sim::deserialize(&bytes).expect("round-trip deserialize");

    assert_eq!(
        restored.checksum(),
        sim.checksum(),
        "round-trip must reproduce the checksum"
    );
    assert_eq!(
        restored.serialize(),
        bytes,
        "re-serializing the restored sim must yield identical bytes"
    );
    assert_eq!(restored.tick_count(), sim.tick_count());
    // The liveness triple round-trips exactly (free-list ORDER and generations preserved).
    assert_eq!(restored.world.free_list(), sim.world.free_list());
    assert_eq!(restored.world.generations(), sim.world.generations());
    assert_eq!(restored.world.capacity(), sim.world.capacity());
}

/// A freshly-built sim (no churn, empty free-list) also round-trips — covers the trivial world.
#[test]
fn snapshot_round_trips_fresh_sim() {
    let sim = Sim::new(42);
    let bytes = sim.serialize();
    let restored = Sim::deserialize(&bytes).expect("fresh sim round-trips");
    assert_eq!(restored.checksum(), sim.checksum());
    assert_eq!(restored.serialize(), bytes);
}

/// The free-list ORDER is load-bearing: it decides the next spawn's slot. A snapshot taken after
/// despawns must resume with spawns landing on exactly the slots the uninterrupted run would use.
#[test]
fn snapshot_preserves_free_list_spawn_order() {
    let mut sim = Sim::new(7);
    // Build a deliberate free-list: spawn 5, despawn #1 then #3 (so free = [1, 3] as a stack).
    let mut ents = Vec::new();
    for _ in 0..5 {
        ents.push(sim.world.spawn());
    }
    sim.world.despawn(ents[1]);
    sim.world.despawn(ents[3]);
    let free_before = sim.world.free_list().to_vec();
    assert_eq!(free_before, vec![1, 3]);

    let restored = Sim::deserialize(&sim.serialize()).expect("deserialize");
    assert_eq!(restored.world.free_list(), free_before);

    // The next spawn on BOTH must reuse the same slot (the free-list's top, 3).
    let mut a = Sim::deserialize(&sim.serialize()).expect("a");
    let mut b = sim;
    let sa = a.world.spawn();
    let sb = b.world.spawn();
    assert_eq!(sa.index, sb.index, "resumed spawn must pick the same slot");
    assert_eq!(sa.index, 3, "the free-list top is slot 3");
}

/// The reader rejects malformed authoritative-snapshot input rather than panicking or silently
/// producing a divergent world (D28 §2). Covers each `DeserializeError` arm reachable at the
/// `Sim::deserialize` boundary plus the `Reader` primitives.
#[test]
fn deserialize_rejects_malformed_input() {
    // `Sim` has no Debug impl, so collapse a deserialize Result to just its error for assertions.
    fn err(bytes: &[u8]) -> DeserializeError {
        Sim::deserialize(bytes)
            .err()
            .expect("expected a decode error")
    }

    // Empty buffer: not even a version byte.
    assert_eq!(err(&[]), DeserializeError::UnexpectedEof);

    // Bad version byte.
    let mut w = Writer::new();
    w.write_u8(99);
    assert_eq!(err(&w.into_bytes()), DeserializeError::BadVersion(99));

    // A valid snapshot with a stray trailing byte must be rejected (format/version skew).
    let sim = Sim::new(1);
    let mut bytes = sim.serialize();
    bytes.push(0xFF);
    assert_eq!(err(&bytes), DeserializeError::TrailingBytes);

    // A truncated valid snapshot (drop the last byte) fails mid-field, never panics.
    let mut bytes = sim.serialize();
    bytes.pop();
    assert!(matches!(
        err(&bytes),
        DeserializeError::UnexpectedEof | DeserializeError::TrailingBytes
    ));

    // A garbage capacity that overruns the buffer is caught before allocating.
    let mut w = Writer::new();
    w.write_u8(1); // version
    w.write_u32(0); // map_id
    w.write_u32(0xFFFF_FFFF); // capacity claims ~4 billion slots
    assert_eq!(err(&w.into_bytes()), DeserializeError::LengthOverflow);

    // The Reader's own primitives reject a short read.
    let mut r = Reader::new(&[0u8, 1]);
    assert_eq!(r.read_u32().unwrap_err(), DeserializeError::UnexpectedEof);
}

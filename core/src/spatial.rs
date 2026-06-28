//! Deterministic uniform-grid spatial index for near-O(1) neighbour queries (invariants #1, #2).
//!
//! Target acquisition was O(n²): every armed `FireAtWill` unit scanned all units each tick
//! ([`combat::acquire_target`](crate::combat)). [`SpatialHash`] buckets live entities by cell
//! so a shooter only examines the cells within its weapon range. The query reproduces the old
//! brute-force scan's pick **bit-for-bit** (min squared distance, ties to the lowest entity
//! index) — it is a pure-performance structure, never a behaviour change.
//!
//! Determinism rules it must hold (the determinism guard greps this file):
//! - Fixed-point / integer cell math only; no floats, no `std`/`libm` transcendentals.
//! - The cell mapping is byte-identical to [`flow_field`](crate::flow_field)/`terrain`, so a
//!   unit's combat-query cell is its pathing/LoS cell — there is no second rounding rule.
//! - No `HashMap`/`HashSet` (their iteration order is nondeterministic). Buckets are `Vec`s
//!   built by ascending slot scan, and the query's comparator is order-independent, so the
//!   result is a pure function of the candidate set — identical across arch and visit order.
//!
//! Like [`flow_field::FlowFieldCache`](crate::flow_field), a `SpatialHash` is rebuilt each tick
//! and is **not** sim state: it is never folded into the checksum.

use crate::components::Vec2;
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::flow_field::{CELL_SIZE, GRID, HALF_EXTENT};

/// Clamp a world coord to `[-HALF_EXTENT, HALF_EXTENT)` then map to a `[0, GRID)` cell index
/// along one axis. A byte-for-byte copy of `flow_field`'s private `axis_to_cell` — keep the two
/// in lockstep so a unit hashes to the same cell it paths/sights from.
#[inline]
fn axis_to_cell(w: Fixed) -> i32 {
    let shifted = w + HALF_EXTENT;
    let c = (shifted / CELL_SIZE).to_int();
    if c < 0 {
        0
    } else if c >= GRID as i32 {
        GRID as i32 - 1
    } else {
        c
    }
}

#[inline]
fn world_to_cell(p: Vec2) -> (i32, i32) {
    (axis_to_cell(p.x), axis_to_cell(p.y))
}

#[inline]
fn in_bounds(cx: i32, cy: i32) -> bool {
    cx >= 0 && cy >= 0 && (cx as usize) < GRID && (cy as usize) < GRID
}

#[inline]
fn cell_index(cx: i32, cy: i32) -> usize {
    (cy as usize) * GRID + (cx as usize)
}

/// A `GRID`×`GRID` uniform grid of entity-slot buckets, rebuilt per tick.
pub struct SpatialHash {
    /// Row-major `GRID*GRID` buckets; each holds entity slot indices in **ascending** order
    /// (we insert by scanning slots `0..capacity` ascending), so no per-query sort is needed.
    buckets: Vec<Vec<usize>>,
}

impl SpatialHash {
    /// Index every live entity slot by its cell, in ascending slot order. O(n) build.
    ///
    /// Inserts **all** live slots (not just units/armed) so the query's candidate universe per
    /// cell matches the old `0..capacity` scan exactly — filtering stays in the caller's
    /// `accept` predicate, so semantics are unchanged. (Narrowing to `EntityKind::Unit` would be
    /// a safe future optimization, but it would change the candidate set this slice must
    /// reproduce verbatim, so it is deliberately left to the `accept` closure.)
    pub fn build(world: &World) -> SpatialHash {
        let mut buckets: Vec<Vec<usize>> = (0..GRID * GRID).map(|_| Vec::new()).collect();
        for i in 0..world.capacity() {
            if !world.is_index_alive(i) {
                continue;
            }
            let (cx, cy) = world_to_cell(world.pos[i]); // already clamped in-bounds
            buckets[cell_index(cx, cy)].push(i);
        }
        SpatialHash { buckets }
    }

    /// The nearest entity to `pos` — minimum squared world distance, ties broken to the lowest
    /// slot index — among the slots within `range` world units that satisfy `accept`. `dist_sq`
    /// supplies each candidate's squared distance to `pos`.
    ///
    /// Reproduces the brute-force `0..capacity` scan's pick **exactly and independent of cell
    /// visitation order**: the comparator is lexicographic on `(dist_sq, idx)`, so equidistant
    /// candidates resolve to the lowest index no matter which cell they fall in. Only cells
    /// within `range` (in cells) of `pos` are visited; every omitted slot is strictly out of
    /// range and `accept` (which owns the authoritative range/LoS test) would reject it anyway.
    pub fn nearest_within(
        &self,
        pos: Vec2,
        range: Fixed,
        accept: impl Fn(usize) -> bool,
        dist_sq: impl Fn(usize) -> Fixed,
    ) -> Option<usize> {
        let (cx, cy) = world_to_cell(pos);
        // A unit within `range` world units is within `range` units on each axis; with
        // CELL_SIZE = 1 that is at most `range.to_int()` whole cells plus one boundary cell from
        // sub-cell offset ⇒ `range.to_int() + 1`. The inclusive `±r_cells` window is therefore a
        // proven superset of the in-range candidates.
        let r_cells = range.to_int() + 1;
        let mut best: Option<(Fixed, usize)> = None; // (dist_sq, idx)
        let mut dy = -r_cells;
        while dy <= r_cells {
            let mut dx = -r_cells;
            while dx <= r_cells {
                let (gx, gy) = (cx + dx, cy + dy);
                if in_bounds(gx, gy) {
                    for &idx in &self.buckets[cell_index(gx, gy)] {
                        if !accept(idx) {
                            continue;
                        }
                        let d = dist_sq(idx);
                        let better = match best {
                            None => true,
                            Some((b_sq, b_idx)) => d < b_sq || (d == b_sq && idx < b_idx),
                        };
                        if better {
                            best = Some((d, idx));
                        }
                    }
                }
                dx += 1;
            }
            dy += 1;
        }
        best.map(|(_, idx)| idx)
    }

    /// Visit every live entity slot in the cell window around `pos` for a `radius`-world-unit area
    /// query, passing each candidate slot to `visit`. Unlike [`nearest_within`](Self::nearest_within)
    /// (which returns the single nearest), this is for **area effects** where every neighbour in
    /// range matters — e.g. proximity suppression ([`combat`](crate::combat) WS-B).
    ///
    /// Like `nearest_within`, the visited set is a proven **superset** of the in-radius slots (the
    /// `±r_cells` window), so the caller MUST apply the authoritative precise test itself — the
    /// squared-distance `<= radius*radius` membership check plus any hostility/kind/liveness filter
    /// — exactly as `acquire_target` leans on `can_engage`. `visit` owns that filter (and any
    /// mutation), which is why this yields raw candidates instead of an accept closure: it lets the
    /// caller hold a single `&mut World` inside `visit` (read position + write the effect) with no
    /// borrow conflict.
    ///
    /// **Determinism contract:** the visitation order is the grid-scan order (`dy` then `dx`), NOT
    /// ascending slot order, so the caller's per-slot effect MUST be order-independent (an
    /// independent saturating accumulate per distinct slot is — invariant #1/#7). Cells are visited
    /// once each and a slot lives in exactly one cell, so no slot is visited twice.
    pub fn for_each_within(&self, pos: Vec2, radius: Fixed, mut visit: impl FnMut(usize)) {
        let (cx, cy) = world_to_cell(pos);
        // Same window proof as `nearest_within`: a slot within `radius` world units is within
        // `radius.to_int() + 1` cells on each axis (CELL_SIZE = 1, plus one boundary cell for the
        // sub-cell offset), so the inclusive `±r_cells` box is a superset of the in-range slots.
        let r_cells = radius.to_int() + 1;
        let mut dy = -r_cells;
        while dy <= r_cells {
            let mut dx = -r_cells;
            while dx <= r_cells {
                let (gx, gy) = (cx + dx, cy + dy);
                if in_bounds(gx, gy) {
                    for &idx in &self.buckets[cell_index(gx, gy)] {
                        visit(idx);
                    }
                }
                dx += 1;
            }
            dy += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Faction, Health, Weapon};
    use crate::ecs::World;

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    fn spawn_at(world: &mut World, x: Fixed, y: Fixed) -> usize {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(x, y);
        i
    }

    #[test]
    fn cell_mapping_matches_flow_field_contract() {
        // Cell `i` covers world `[-HALF_EXTENT + i, -HALF_EXTENT + i + 1)`; origin sits at
        // cell GRID/2. A unit on a cell boundary maps deterministically (truncating floor).
        assert_eq!(axis_to_cell(fx(0)), (GRID / 2) as i32);
        assert_eq!(axis_to_cell(fx(-(GRID as i32) / 2)), 0);
        assert_eq!(axis_to_cell(fx(1000)), GRID as i32 - 1); // clamped to border
        assert_eq!(axis_to_cell(fx(-1000)), 0);
    }

    #[test]
    fn nearest_within_picks_min_distance() {
        let mut world = World::new();
        let near = spawn_at(&mut world, fx(2), fx(0));
        let far = spawn_at(&mut world, fx(20), fx(0));
        let origin = Vec2::new(fx(0), fx(0));
        let hash = SpatialHash::build(&world);
        let got = hash.nearest_within(
            origin,
            fx(50),
            |_| true,
            |i| (world.pos[i] - origin).len_sq(),
        );
        assert_eq!(got, Some(near));
        let _ = far;
    }

    #[test]
    fn nearest_within_respects_accept_filter() {
        let mut world = World::new();
        let _rejected = spawn_at(&mut world, fx(2), fx(0));
        let accepted = spawn_at(&mut world, fx(5), fx(0));
        let origin = Vec2::new(fx(0), fx(0));
        let hash = SpatialHash::build(&world);
        // accept only the farther slot → it must win despite being farther.
        let got = hash.nearest_within(
            origin,
            fx(50),
            |i| i == accepted,
            |i| (world.pos[i] - origin).len_sq(),
        );
        assert_eq!(got, Some(accepted));
    }

    #[test]
    fn nearest_within_ties_to_lowest_index_across_buckets() {
        let mut world = World::new();
        // Three equidistant (distance 5) candidates in DIFFERENT cells/buckets.
        let low = spawn_at(&mut world, fx(5), fx(0));
        let mid = spawn_at(&mut world, fx(0), fx(5));
        let high = spawn_at(&mut world, fx(-5), fx(0));
        assert!(low < mid && mid < high);
        let origin = Vec2::new(fx(0), fx(0));
        let hash = SpatialHash::build(&world);
        let got = hash.nearest_within(
            origin,
            fx(50),
            |_| true,
            |i| (world.pos[i] - origin).len_sq(),
        );
        assert_eq!(got, Some(low), "lowest index must win a tie regardless of cell order");
    }

    #[test]
    fn empty_world_yields_none() {
        let world = World::new();
        let hash = SpatialHash::build(&world);
        let got = hash.nearest_within(
            Vec2::new(fx(0), fx(0)),
            fx(10),
            |_| true,
            |_| Fixed::ZERO,
        );
        assert_eq!(got, None);
    }

    #[test]
    fn for_each_within_visits_a_superset_of_the_in_radius_slots() {
        let mut world = World::new();
        let origin = Vec2::new(fx(0), fx(0));
        let inside_a = spawn_at(&mut world, fx(2), fx(0)); // dist 2
        let inside_b = spawn_at(&mut world, fx(0), fx(3)); // dist 3
        let outside = spawn_at(&mut world, fx(30), fx(0)); // dist 30, far outside
        let hash = SpatialHash::build(&world);
        let radius = fx(4);
        let r_sq = radius * radius;
        // Caller applies the precise squared-distance filter inside `visit` (the API contract).
        let mut matched: Vec<usize> = Vec::new();
        let mut visited: Vec<usize> = Vec::new();
        hash.for_each_within(origin, radius, |idx| {
            visited.push(idx);
            if (world.pos[idx] - origin).len_sq() <= r_sq {
                matched.push(idx);
            }
        });
        matched.sort_unstable();
        assert_eq!(matched, vec![inside_a, inside_b], "precise filter yields exactly the in-radius slots");
        // Superset property: every in-radius slot was visited; `outside` may or may not be visited
        // (it is far enough to fall outside the cell window here), but must never be a false match.
        assert!(visited.contains(&inside_a) && visited.contains(&inside_b), "all in-radius slots visited");
        assert!(!matched.contains(&outside), "out-of-radius slot is never a match");
    }

    #[test]
    fn for_each_within_visits_each_slot_at_most_once() {
        let mut world = World::new();
        let a = spawn_at(&mut world, fx(1), fx(1));
        let b = spawn_at(&mut world, fx(-1), fx(2));
        let hash = SpatialHash::build(&world);
        let mut counts = std::collections::BTreeMap::new();
        hash.for_each_within(Vec2::new(fx(0), fx(0)), fx(5), |idx| {
            *counts.entry(idx).or_insert(0) += 1;
        });
        assert_eq!(counts.get(&a), Some(&1), "a visited exactly once");
        assert_eq!(counts.get(&b), Some(&1), "b visited exactly once");
    }

    /// Build a brute-force `min dist_sq, lowest-index` oracle and assert the spatial query
    /// agrees for every shooter across a seeded mixed-position field — the load-bearing
    /// equivalence guard (extends to combat's `can_engage` filter in `combat.rs` tests).
    #[test]
    fn spatial_query_matches_brute_force_over_seeded_field() {
        use crate::rng::Rng;
        let mut rng = Rng::new(0xA5_5A_1234);
        let mut world = World::new();
        // 120 units at seeded integer + quarter-cell-fractional positions in [-60, 60).
        for _ in 0..120 {
            let x = fx(rng.below(120) as i32 - 60) + Fixed::from_ratio(rng.below(4) as i32, 4);
            let y = fx(rng.below(120) as i32 - 60) + Fixed::from_ratio(rng.below(4) as i32, 4);
            let _ = spawn_at(&mut world, x, y);
            // Mark all alive with a faction/health so they're real candidates.
            let i = world.capacity() - 1;
            world.faction[i] = Faction::Enemy;
            world.health[i] = Health::full(fx(100));
            world.weapon[i] = Weapon::default();
        }
        let hash = SpatialHash::build(&world);
        let range = fx(40);
        for shooter in 0..world.capacity() {
            if !world.is_index_alive(shooter) {
                continue;
            }
            let pos = world.pos[shooter];
            let accept = |i: usize| {
                i != shooter
                    && world.is_index_alive(i)
                    && (world.pos[i] - pos).len_sq() <= range * range
            };
            let dist = |i: usize| (world.pos[i] - pos).len_sq();
            // Brute-force oracle: ascending scan, strictly-less replacement (lowest index on tie).
            let mut want: Option<(usize, Fixed)> = None;
            for t in 0..world.capacity() {
                if !accept(t) {
                    continue;
                }
                let d = dist(t);
                match want {
                    Some((_, b)) if d >= b => {}
                    _ => want = Some((t, d)),
                }
            }
            let got = hash.nearest_within(pos, range, accept, dist);
            assert_eq!(got, want.map(|(i, _)| i), "shooter {shooter}: spatial != brute-force");
        }
    }
}

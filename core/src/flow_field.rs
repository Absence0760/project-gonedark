//! Deterministic flow-field pathing (invariants #1, #4 — fixed-point, no floats).
//!
//! A [`FlowField`] is an integer cost map over a fixed square grid of the playfield.
//! Costs are filled by an integer Dijkstra expansion *outward from the goal cell*
//! (8-connected, orthogonal step 10 / diagonal 14 ≈ `√2·10`), so every cell records the
//! cheapest hop-distance to the goal. To move, a unit [`sample`](FlowField::sample)s the
//! field at its world position and gets the unit direction toward the lowest-cost
//! neighbouring cell — i.e. downhill toward the goal.
//!
//! Phase 1 has no obstacles (open field), so the field degenerates to "point at the
//! goal", which is exactly correct; the *structure* is what build-order step 3 calls for
//! and generalises to Phase 2 cover/terrain by raising per-cell entry costs. Everything is
//! integer or [`Fixed`]; the only direction-producing step is [`Vec2::normalized`], which
//! is fixed-point. Iteration order is fixed and documented at every tie, so the field is
//! bit-identical across arches.
//!
//! **Caching (Phase 3).** A field is a *pure function of its goal*, so units heading to the
//! same target can share one build. [`FlowFieldCache`] memoises one field per distinct goal
//! within a tick; the result a unit samples is bit-identical to building its own field, but a
//! 200-unit push to a shared objective builds a handful of fields instead of ~200 (the per-unit
//! rebuild was the measured 60 Hz bottleneck — see `docs/plans/phase-3-plan.md` §"Workstream A").

use crate::components::Vec2;
use crate::fixed::Fixed;

/// Cells per axis. The grid is `GRID`×`GRID` square.
pub const GRID: usize = 128;

/// World-space side length of one cell (1 world unit). Centred so the playfield spans
/// world coords `[-HALF_EXTENT, HALF_EXTENT)` on each axis (here `[-64, 64)`).
pub const CELL_SIZE: Fixed = Fixed::ONE;

/// Half the playfield extent in world units (`GRID/2 * CELL_SIZE`). The grid covers
/// `[-HALF_EXTENT, HALF_EXTENT)` on x and y; positions outside are clamped to the border.
pub const HALF_EXTENT: Fixed = Fixed::from_int((GRID / 2) as i32);

/// Integer step cost for an orthogonal hop.
const COST_ORTHO: u32 = 10;
/// Integer step cost for a diagonal hop (≈ `√2 · 10`, the standard 14 approximation).
const COST_DIAG: u32 = 14;

/// Sentinel "unreached" cost. The field is fully connected in Phase 1 (no obstacles), so
/// this only ever remains for the goal's own initialisation step, never after a build.
const UNREACHED: u32 = u32::MAX;

/// The eight neighbour offsets, in a FIXED scan order. This order is the sole tie-break
/// for equal-cost neighbours in both the build and [`sample`](FlowField::sample), so it is
/// load-bearing for determinism — do not reorder. Orthogonals first (N, E, S, W), then
/// diagonals (NE, SE, SW, NW); each entry carries its integer step cost.
const NEIGHBORS: [(i32, i32, u32); 8] = [
    (0, 1, COST_ORTHO),  // N
    (1, 0, COST_ORTHO),  // E
    (0, -1, COST_ORTHO), // S
    (-1, 0, COST_ORTHO), // W
    (1, 1, COST_DIAG),   // NE
    (1, -1, COST_DIAG),  // SE
    (-1, -1, COST_DIAG), // SW
    (-1, 1, COST_DIAG),  // NW
];

/// An integer cost field over the playfield grid, lowest at the goal cell.
pub struct FlowField {
    /// Row-major `GRID*GRID` cost-to-goal per cell. Lower is closer to the goal.
    cost: Vec<u32>,
    /// Goal cell, used to detect "already in the goal cell" for the final approach.
    goal_cx: i32,
    goal_cy: i32,
    /// The exact world-space goal target. Within the goal cell, units steer straight at
    /// *this* (not the cell centre) so the approach lands on the real target, not a cell.
    goal: Vec2,
}

impl FlowField {
    /// Build the field by an integer Dijkstra expansion outward from `goal`'s cell, using
    /// a bucket queue (Dial's algorithm) keyed by integer cost.
    ///
    /// Deterministic by construction: cells are processed in nondecreasing cost order, and
    /// within one cost bucket in first-inserted order. Insertion order is fully determined
    /// by the fixed `NEIGHBORS` scan order applied to cells already drained in that same
    /// order — no priority-queue address/hash ordering, no float keys. With no obstacles
    /// this fills the whole grid with a tidy distance gradient. O(N) in cell count.
    pub fn build(goal: Vec2) -> FlowField {
        let (goal_cx, goal_cy) = world_to_cell(goal);
        let mut cost = vec![UNREACHED; GRID * GRID];

        // Bucket queue: `buckets[c]` holds cell indices first reached at cost `c`. The
        // maximum reachable cost on an open grid is bounded by the longest 8-connected
        // path; (GRID-1) diagonal hops + a few orthogonals is a safe upper bound.
        let max_cost = (GRID as u32) * COST_DIAG + COST_ORTHO;
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); (max_cost + 1) as usize];

        let goal_idx = cell_index(goal_cx, goal_cy);
        cost[goal_idx] = 0;
        buckets[0].push(goal_idx as u32);

        // Drain buckets in increasing cost. A cell may be queued more than once (relaxed to
        // a lower cost); the `c != cost[idx]` guard skips stale entries, so each cell is
        // expanded exactly once at its final cost — order stays deterministic.
        for c in 0..buckets.len() {
            // Index manually: relaxing may push into a *later* bucket (always c+step > c),
            // never the current or an earlier one, so the snapshot length here is final for
            // bucket c by the time we reach it.
            let mut k = 0;
            while k < buckets[c].len() {
                let idx = buckets[c][k] as usize;
                k += 1;
                if cost[idx] != c as u32 {
                    continue; // stale duplicate; the live copy was expanded already
                }
                let cx = (idx % GRID) as i32;
                let cy = (idx / GRID) as i32;
                for &(dx, dy, step) in NEIGHBORS.iter() {
                    let nx = cx + dx;
                    let ny = cy + dy;
                    if !in_bounds(nx, ny) {
                        continue;
                    }
                    let nidx = cell_index(nx, ny);
                    let cand = c as u32 + step;
                    if cand < cost[nidx] {
                        cost[nidx] = cand;
                        buckets[cand as usize].push(nidx as u32);
                    }
                }
            }
        }

        FlowField {
            cost,
            goal_cx,
            goal_cy,
            goal,
        }
    }

    /// Cost-to-goal at a world position (its cell), clamped to the grid border.
    #[inline]
    pub fn cost_at(&self, pos: Vec2) -> u32 {
        let (cx, cy) = world_to_cell(pos);
        self.cost[cell_index(cx, cy)]
    }

    /// Unit flow direction at a world position: toward the lowest-cost neighbour cell.
    ///
    /// Positions outside the grid clamp to the border cell. When the position is already in
    /// the goal cell (or no neighbour is strictly cheaper), steer straight at the true goal
    /// target so discretisation can't make a unit orbit a cell — the caller's arrival snap
    /// then finishes the approach. The returned direction is a fixed-point unit vector via
    /// [`Vec2::normalized`]; a zero result means "already there".
    pub fn sample(&self, pos: Vec2) -> Vec2 {
        let (cx, cy) = world_to_cell(pos);
        let here = self.cost[cell_index(cx, cy)];

        // Find the cheapest neighbour; ties break to the earliest in NEIGHBORS scan order.
        let mut best = here;
        let mut best_dir: Option<(i32, i32)> = None;
        for &(dx, dy, _step) in NEIGHBORS.iter() {
            let nx = cx + dx;
            let ny = cy + dy;
            if !in_bounds(nx, ny) {
                continue;
            }
            let c = self.cost[cell_index(nx, ny)];
            if c < best {
                best = c;
                best_dir = Some((dx, dy));
            }
        }

        // In the goal cell, or nothing strictly downhill: aim at the true goal target so
        // the final approach is exact, not snapped to a cell direction. (Aiming at the
        // cell centre instead would let a unit orbit the cell centre forever.)
        if best_dir.is_none() || (cx == self.goal_cx && cy == self.goal_cy) {
            return (self.goal - pos).normalized();
        }

        let (dx, dy) = best_dir.unwrap();
        Vec2::new(Fixed::from_int(dx), Fixed::from_int(dy)).normalized()
    }
}

/// A per-tick memo of flow fields, keyed by goal. A [`FlowField`] is a pure function of its
/// goal, so two units with the same target share one build — what each samples is bit-identical
/// to having built its own field. Create one per tick (goals change as orders do) and drop it at
/// tick end; nothing in the cache crosses tick boundaries, so it is not sim state and never
/// touches the checksum.
///
/// Determinism: the cache is only ever *probed* (`get`), never iterated for behaviour, so there
/// is no hash-order hazard — and the value returned is independent of insertion order. The store
/// is a flat `Vec` linear-probed because the number of distinct goals live in one tick is small
/// (a handful), so a map's overhead/iteration-order questions buy nothing.
#[derive(Default)]
pub struct FlowFieldCache {
    fields: Vec<(Vec2, FlowField)>,
}

impl FlowFieldCache {
    /// An empty cache for a fresh tick.
    pub fn new() -> Self {
        FlowFieldCache { fields: Vec::new() }
    }

    /// The field for `goal`, building and memoising it on first request this tick. The returned
    /// reference is bit-identical to `FlowField::build(goal)` regardless of how many callers
    /// shared it.
    pub fn get(&mut self, goal: Vec2) -> &FlowField {
        // Resolve the index with no live borrow outstanding, then return — avoids the
        // conditional-return-of-borrow borrowck snag.
        let idx = match self.fields.iter().position(|(g, _)| *g == goal) {
            Some(i) => i,
            None => {
                self.fields.push((goal, FlowField::build(goal)));
                self.fields.len() - 1
            }
        };
        &self.fields[idx].1
    }

    /// Number of distinct goals built this tick (the dedup factor). Test-only.
    #[cfg(test)]
    pub(crate) fn distinct_goals(&self) -> usize {
        self.fields.len()
    }
}

/// Clamp a world coord to `[-HALF_EXTENT, HALF_EXTENT)` then map to a `[0, GRID)` cell
/// index along one axis. Cell `i` covers world `[-HALF_EXTENT + i, -HALF_EXTENT + i + 1)`.
#[inline]
fn axis_to_cell(w: Fixed) -> i32 {
    // Shift into [0, GRID*CELL_SIZE) world units, truncate toward -inf to a cell, clamp.
    let shifted = w + HALF_EXTENT;
    // CELL_SIZE is 1 world unit, so the integer part is the cell. Use to_int (floor for
    // these non-negative-after-shift values; negatives are clamped to 0 below anyway).
    let c = (shifted / CELL_SIZE).to_int();
    if c < 0 {
        0
    } else if c >= GRID as i32 {
        GRID as i32 - 1
    } else {
        c
    }
}

/// World position → clamped `(cell_x, cell_y)`.
#[inline]
fn world_to_cell(p: Vec2) -> (i32, i32) {
    (axis_to_cell(p.x), axis_to_cell(p.y))
}

/// Is `(cx, cy)` a valid cell?
#[inline]
fn in_bounds(cx: i32, cy: i32) -> bool {
    cx >= 0 && cy >= 0 && (cx as usize) < GRID && (cy as usize) < GRID
}

/// Row-major flat index for an in-bounds cell.
#[inline]
fn cell_index(cx: i32, cy: i32) -> usize {
    (cy as usize) * GRID + (cx as usize)
}

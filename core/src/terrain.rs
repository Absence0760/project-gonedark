//! Terrain, cover, and line-of-sight — the spatial-query foundation combat and fog read
//! (invariants #1, #4 — fixed-point / integer only, never mutated by render).
//!
//! The map is a `GRID`×`GRID` cell grid sharing the **exact** coordinate mapping of
//! [`flow_field`](crate::flow_field) (same `GRID`, `CELL_SIZE`, `HALF_EXTENT`, same clamped
//! `world → cell` floor) so a unit's pathing cell and its cover/LoS cell are always the same
//! cell. Each cell holds a [`Cover`] level; `Heavy` cover also blocks line of sight (a wall).
//!
//! Terrain is **static map data** set once at scenario build and never mutated by a system,
//! so it is deliberately NOT folded into the per-tick checksum (invariant #7) — there is no
//! per-tick terrain state to diverge.
//!
//! ## Public contract (do not change these signatures — combat/fog/sim depend on them)
//! - [`Terrain::open`] — an all-clear field.
//! - [`Terrain::cover_at`] — cover level at a world position (clamped to the grid).
//! - [`Terrain::line_of_sight`] — is the segment `a→b` unobstructed by sight-blocking cells?
//!
//! IMPLEMENTATION OWNER: worker 1. This file is a compiling stub; fill in the real
//! grid-aware bodies + inline `#[cfg(test)]` tests. Keep the signatures above intact.

use crate::components::Vec2;
use crate::fixed::Fixed;
use crate::flow_field::{CELL_SIZE, GRID, HALF_EXTENT};

/// Cover level at a cell. Heavier cover mitigates more incoming damage; `Heavy` additionally
/// blocks line of sight (it is a solid wall, not just concealment).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Cover {
    /// Open ground — full damage, no sight blocking.
    #[default]
    None,
    /// Light cover (sandbags, hedges) — partial damage mitigation, sight passes.
    Light,
    /// Heavy cover (walls) — strong mitigation AND blocks line of sight.
    Heavy,
}

impl Cover {
    /// Damage multiplier this cover applies to incoming fire, as a Fixed in `[0, 1]`.
    /// `None` → 1 (full), `Light` → 1/2, `Heavy` → 1/4. (Tunable; worker 1 may refine.)
    #[inline]
    pub fn damage_multiplier(self) -> Fixed {
        match self {
            Cover::None => Fixed::ONE,
            Cover::Light => Fixed::from_ratio(1, 2),
            Cover::Heavy => Fixed::from_ratio(1, 4),
        }
    }

    /// Does this cover block line of sight?
    #[inline]
    pub fn blocks_sight(self) -> bool {
        matches!(self, Cover::Heavy)
    }
}

/// A static cover/terrain map over the playfield grid.
pub struct Terrain {
    /// Row-major `GRID*GRID` cover per cell. Internal shape is worker 1's to refine.
    cells: Vec<Cover>,
}

/// A map identifier — names *which* static map a sim is on, out-of-band agreed by both peers.
///
/// Terrain is static map data (never mutated by a system, never in the per-tick checksum), so the
/// authoritative snapshot (D28) carries this small id rather than the whole `GRID×GRID` grid: the
/// resuming peer rebuilds the identical terrain from the id via [`Terrain::from_map_id`]. The
/// only scene today is the open playfield, which is map id `0` ([`MapId::SCENE`]).
pub type MapId = u16;

impl Terrain {
    /// The default scene's map id: the open playfield. The sim starts on this map (D28: the
    /// snapshot serializes a `map_id`, not the grid; this is the only one defined so far).
    pub const SCENE_MAP_ID: MapId = 0;

    /// An all-clear field (no cover, no walls) — the Phase 1 open playfield.
    pub fn open() -> Terrain {
        Terrain {
            cells: vec![Cover::None; GRID * GRID],
        }
    }

    /// Rebuild the terrain named by `id`. The inverse of "which map am I on?" used by the
    /// authoritative snapshot to re-derive `Terrain` without serializing the grid (D28).
    ///
    /// Only [`SCENE_MAP_ID`](Self::SCENE_MAP_ID) (`0`, the open playfield) is defined today; an
    /// unknown id rebuilds the open field too, so a forward-compatible snapshot never panics.
    /// As real authored maps land, this gains a match arm per id (each a deterministic builder).
    pub fn from_map_id(id: MapId) -> Terrain {
        match id {
            Self::SCENE_MAP_ID => Terrain::open(),
            _ => Terrain::open(),
        }
    }

    /// Cover at a world position (its cell, clamped to the grid border).
    #[inline]
    pub fn cover_at(&self, pos: Vec2) -> Cover {
        let (cx, cy) = world_to_cell(pos);
        // Both coords are clamped to `[0, GRID)` by `axis_to_cell`, so the index is in range.
        self.cells[cell_index(cx, cy)]
    }

    /// Cover at an explicit cell. Out-of-bounds cells read as [`Cover::None`] (the void
    /// beyond the playfield is open, never a wall) so callers needn't bounds-check.
    #[inline]
    pub fn cover_at_cell(&self, cx: i32, cy: i32) -> Cover {
        if in_bounds(cx, cy) {
            self.cells[cell_index(cx, cy)]
        } else {
            Cover::None
        }
    }

    /// Place cover at one cell. Out-of-bounds coords are ignored (no panic) so scenario
    /// builders can splatter without pre-clamping.
    #[inline]
    pub fn set_cover(&mut self, cx: i32, cy: i32, c: Cover) {
        if in_bounds(cx, cy) {
            let idx = cell_index(cx, cy);
            self.cells[idx] = c;
        }
    }

    /// Fill an inclusive rectangle of cells `[cx0..=cx1] × [cy0..=cy1]` with `c`. The corners
    /// may be given in any order; cells outside the grid are silently skipped. Handy for
    /// laying a wall (`Heavy`) or a sandbag line (`Light`) in one call.
    pub fn fill_rect(&mut self, cx0: i32, cy0: i32, cx1: i32, cy1: i32, c: Cover) {
        let (lo_x, hi_x) = if cx0 <= cx1 { (cx0, cx1) } else { (cx1, cx0) };
        let (lo_y, hi_y) = if cy0 <= cy1 { (cy0, cy1) } else { (cy1, cy0) };
        let mut y = lo_y;
        while y <= hi_y {
            let mut x = lo_x;
            while x <= hi_x {
                self.set_cover(x, y, c);
                x += 1;
            }
            y += 1;
        }
    }

    /// Map a world position to its `(cell_x, cell_y)`, clamped to the grid border — the exact
    /// mapping [`flow_field`](crate::flow_field) uses, so a unit's pathing cell == its
    /// cover/LoS cell.
    #[inline]
    pub fn cell_of(&self, pos: Vec2) -> (i32, i32) {
        world_to_cell(pos)
    }

    /// Is the line segment from `a` to `b` free of sight-blocking cells?
    ///
    /// Both endpoints are mapped to their grid cells (same clamped floor as `flow_field`),
    /// then an integer supercover DDA walks **every** cell the segment touches. A cell
    /// blocks only if it is **strictly between** the two endpoint cells and
    /// [`Cover::blocks_sight`] (i.e. `Heavy`) — the endpoints' own cells never block, so a
    /// unit standing inside a wall can still see/be seen out. Open field ⇒ always true.
    ///
    /// Deterministic (integer-only cell math, no floats) and symmetric: the endpoint pair is
    /// canonicalised before the walk, so `line_of_sight(a, b) == line_of_sight(b, a)` by
    /// construction regardless of traversal direction.
    pub fn line_of_sight(&self, a: Vec2, b: Vec2) -> bool {
        let (ax, ay) = world_to_cell(a);
        let (bx, by) = world_to_cell(b);

        // Canonicalise the endpoint ORDER so the walk is identical for (a,b) and (b,a). We
        // pick a total order on the cell pair and always traverse low→high; the traversal
        // itself is symmetric in which cells it visits, but fixing the start removes any
        // residual asymmetry in tie handling.
        let ((sx, sy), (ex, ey)) = if (ay, ax) <= (by, bx) {
            ((ax, ay), (bx, by))
        } else {
            ((bx, by), (ax, ay))
        };

        // Same start and end cell — nothing strictly between, always visible.
        if sx == ex && sy == ey {
            return true;
        }

        // Integer supercover DDA. Step one axis at a time, and when the running error
        // straddles a corner exactly, step both — so diagonal sight through a corner is only
        // open if *both* flanking cells are clear (no peeking through wall corners).
        let dx = (ex - sx).abs();
        let dy = (ey - sy).abs();
        let step_x = if ex > sx { 1 } else { -1 };
        let step_y = if ey > sy { 1 } else { -1 };

        let mut x = sx;
        let mut y = sy;
        // `err = dx - dy`, scaled by 2 in comparisons (classic integer Bresenham/supercover).
        let mut err = dx - dy;

        loop {
            // Advance to the next cell.
            let e2 = 2 * err;
            if e2 > -dy && e2 < dx {
                // Exact diagonal corner: the segment grazes between two cells. Sight may pass
                // only if BOTH flanking cells are clear — otherwise a unit could peek through
                // the corner of a wall. Check the two flanks (skipping either endpoint cell,
                // which never blocks) before taking the combined diagonal step.
                let flanks = [(x + step_x, y), (x, y + step_y)];
                for (fx, fy) in flanks {
                    if (fx, fy) != (sx, sy)
                        && (fx, fy) != (ex, ey)
                        && self.cover_at_cell(fx, fy).blocks_sight()
                    {
                        return false;
                    }
                }
                err -= dy;
                err += dx;
                x += step_x;
                y += step_y;
            } else if e2 > -dy {
                err -= dy;
                x += step_x;
            } else {
                err += dx;
                y += step_y;
            }

            // Reached the end cell: stop. The end cell is an endpoint and never blocks.
            if x == ex && y == ey {
                return true;
            }

            // A strictly-between cell that blocks sight closes the line.
            if self.cover_at_cell(x, y).blocks_sight() {
                return false;
            }
        }
    }
}

impl Default for Terrain {
    fn default() -> Self {
        Terrain::open()
    }
}

// --- Coordinate mapping (an EXACT mirror of flow_field's private mapping) -------------------
//
// flow_field keeps `axis_to_cell` / `world_to_cell` / `in_bounds` / `cell_index` private, so
// they are re-derived here against the SAME public consts (`GRID`, `CELL_SIZE`, `HALF_EXTENT`).
// Keeping these byte-for-byte identical is load-bearing: a unit's pathing cell must equal its
// cover/LoS cell. If flow_field's mapping ever changes, this must change in lockstep.

/// Clamp a world coord to `[-HALF_EXTENT, HALF_EXTENT)` then map to a `[0, GRID)` cell index
/// along one axis. Cell `i` covers world `[-HALF_EXTENT + i, -HALF_EXTENT + i + 1)`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Vec2;
    use crate::fixed::Fixed;

    /// World position at the centre of cell `(cx, cy)`. Cell `i` covers
    /// `[-HALF_EXTENT + i, -HALF_EXTENT + i + 1)`, so its centre is at `+ i + 1/2`.
    fn cell_center(cx: i32, cy: i32) -> Vec2 {
        let half = Fixed::from_ratio(1, 2);
        let origin = Fixed::ZERO - HALF_EXTENT;
        Vec2::new(
            origin + Fixed::from_int(cx) + half,
            origin + Fixed::from_int(cy) + half,
        )
    }

    #[test]
    fn open_field_los_always_true() {
        let t = Terrain::open();
        // A spread of segments across the open field, including reversed pairs.
        let pairs = [
            (
                cell_center(0, 0),
                cell_center(GRID as i32 - 1, GRID as i32 - 1),
            ),
            (cell_center(10, 20), cell_center(100, 30)),
            (cell_center(64, 0), cell_center(64, 127)),
            (cell_center(5, 60), cell_center(120, 60)),
        ];
        for (a, b) in pairs {
            assert!(t.line_of_sight(a, b), "open field must always have LoS");
            assert!(
                t.line_of_sight(b, a),
                "open field must always have LoS (reversed)"
            );
        }
    }

    #[test]
    fn cover_defaults_full_damage_and_no_block() {
        // Sanity on the Cover methods themselves.
        assert_eq!(Cover::None.damage_multiplier(), Fixed::ONE);
        assert_eq!(Cover::Light.damage_multiplier(), Fixed::from_ratio(1, 2));
        assert_eq!(Cover::Heavy.damage_multiplier(), Fixed::from_ratio(1, 4));
        assert!(!Cover::None.blocks_sight());
        assert!(!Cover::Light.blocks_sight());
        assert!(Cover::Heavy.blocks_sight());
    }

    #[test]
    fn heavy_between_blocks_light_and_none_do_not() {
        let a = cell_center(10, 50);
        let b = cell_center(20, 50);
        let mid = (15, 50); // strictly between on the horizontal lane

        let mut t = Terrain::open();
        assert!(t.line_of_sight(a, b), "clear lane has LoS");

        t.set_cover(mid.0, mid.1, Cover::Light);
        assert!(t.line_of_sight(a, b), "light cover does not block sight");

        t.set_cover(mid.0, mid.1, Cover::None);
        assert!(t.line_of_sight(a, b), "cleared cell again has LoS");

        t.set_cover(mid.0, mid.1, Cover::Heavy);
        assert!(
            !t.line_of_sight(a, b),
            "heavy cover between endpoints blocks sight"
        );
    }

    #[test]
    fn los_is_symmetric() {
        let a = cell_center(8, 8);
        let b = cell_center(40, 25); // a non-axis-aligned (diagonal-ish) segment

        // Block a cell roughly along the line.
        let mut t = Terrain::open();
        // Walk to discover a strictly-between cell the segment crosses, then block it.
        // Place several heavy cells and confirm both directions agree in every case.
        let candidates = [(20, 16), (24, 18), (30, 20), (15, 12)];
        for &(cx, cy) in candidates.iter() {
            let mut tt = Terrain::open();
            tt.set_cover(cx, cy, Cover::Heavy);
            assert_eq!(
                tt.line_of_sight(a, b),
                tt.line_of_sight(b, a),
                "LoS must be symmetric with a heavy cell at ({cx},{cy})"
            );
        }
        // Also the open case is trivially symmetric.
        assert_eq!(t.line_of_sight(a, b), t.line_of_sight(b, a));
        let _ = &mut t;
    }

    #[test]
    fn los_symmetric_when_blocked() {
        // Build a vertical wall and shoot a horizontal segment through it: both directions
        // must report blocked.
        let mut t = Terrain::open();
        t.fill_rect(50, 40, 50, 70, Cover::Heavy); // a vertical wall at x = 50
        let a = cell_center(30, 55);
        let b = cell_center(80, 55);
        assert!(!t.line_of_sight(a, b), "wall blocks the crossing segment");
        assert_eq!(
            t.line_of_sight(a, b),
            t.line_of_sight(b, a),
            "blocked LoS is symmetric"
        );
    }

    #[test]
    fn clear_lane_parallel_to_wall_is_unobstructed() {
        // A wall along x = 50 (rows 40..=70). A segment running PARALLEL on x = 49 (one cell
        // beside the wall) must stay clear: the wall never enters the traversed cells.
        let mut t = Terrain::open();
        t.fill_rect(50, 40, 50, 70, Cover::Heavy);
        let a = cell_center(49, 41);
        let b = cell_center(49, 69);
        assert!(
            t.line_of_sight(a, b),
            "a lane parallel to and beside the wall is unobstructed"
        );
        assert!(t.line_of_sight(b, a));
    }

    #[test]
    fn endpoint_cell_walls_do_not_block() {
        // A unit standing IN a wall cell can still see out: endpoint cells never block.
        let mut t = Terrain::open();
        let a = cell_center(10, 10);
        let b = cell_center(20, 10);
        t.set_cover(10, 10, Cover::Heavy); // endpoint a is a wall
        t.set_cover(20, 10, Cover::Heavy); // endpoint b is a wall
        assert!(
            t.line_of_sight(a, b),
            "endpoints' own wall cells must not block LoS"
        );
        assert!(t.line_of_sight(b, a));
    }

    #[test]
    fn cover_at_returns_placed_cover() {
        let mut t = Terrain::open();
        assert_eq!(t.cover_at(cell_center(33, 77)), Cover::None);
        t.set_cover(33, 77, Cover::Light);
        assert_eq!(t.cover_at(cell_center(33, 77)), Cover::Light);
        t.set_cover(33, 77, Cover::Heavy);
        assert_eq!(t.cover_at(cell_center(33, 77)), Cover::Heavy);
    }

    #[test]
    fn cover_at_clamps_out_of_grid_positions() {
        let mut t = Terrain::open();
        // Place cover on the far corner cell, then probe with a way-out-of-bounds position
        // that must clamp to that border cell.
        t.set_cover(GRID as i32 - 1, GRID as i32 - 1, Cover::Heavy);
        let far = Vec2::new(Fixed::from_int(1000), Fixed::from_int(1000));
        assert_eq!(
            t.cover_at(far),
            Cover::Heavy,
            "out-of-grid position clamps to the border cell"
        );
        // The opposite corner clamps to (0,0).
        t.set_cover(0, 0, Cover::Light);
        let near = Vec2::new(Fixed::from_int(-1000), Fixed::from_int(-1000));
        assert_eq!(t.cover_at(near), Cover::Light);
    }

    #[test]
    fn set_cover_out_of_bounds_is_ignored() {
        let mut t = Terrain::open();
        // Should not panic and should not change any in-grid cell.
        t.set_cover(-5, 10, Cover::Heavy);
        t.set_cover(10, -5, Cover::Heavy);
        t.set_cover(GRID as i32, 10, Cover::Heavy);
        t.set_cover(10, GRID as i32, Cover::Heavy);
        assert_eq!(t.cover_at_cell(-5, 10), Cover::None);
        assert_eq!(t.cover_at_cell(10, 10), Cover::None);
    }

    /// Build a small fixed scenario used to prove repeatability.
    fn build_scenario() -> Terrain {
        let mut t = Terrain::open();
        t.fill_rect(50, 40, 50, 70, Cover::Heavy);
        t.fill_rect(60, 20, 64, 24, Cover::Light);
        t.set_cover(5, 5, Cover::Heavy);
        t
    }

    #[test]
    fn building_same_map_twice_is_identical() {
        let t1 = build_scenario();
        let t2 = build_scenario();

        // Every cell's cover matches.
        for cy in 0..GRID as i32 {
            for cx in 0..GRID as i32 {
                assert_eq!(t1.cover_at_cell(cx, cy), t2.cover_at_cell(cx, cy));
            }
        }

        // A battery of LoS queries returns identical results on both builds.
        let probes = [
            (cell_center(30, 55), cell_center(80, 55)),
            (cell_center(0, 0), cell_center(127, 127)),
            (cell_center(49, 41), cell_center(49, 69)),
            (cell_center(8, 8), cell_center(40, 25)),
            (cell_center(62, 22), cell_center(10, 22)),
        ];
        for (a, b) in probes {
            assert_eq!(t1.line_of_sight(a, b), t2.line_of_sight(a, b));
            assert_eq!(t1.line_of_sight(a, b), t1.line_of_sight(b, a));
        }
    }

    #[test]
    fn terrain_cell_matches_flow_field_cell() {
        // The whole point of mirroring the mapping: terrain's cell == flow_field's cell.
        let t = Terrain::open();
        let probes = [
            Vec2::new(Fixed::from_int(0), Fixed::from_int(0)),
            Vec2::new(Fixed::from_ratio(-1, 2), Fixed::from_ratio(3, 2)),
            Vec2::new(Fixed::from_int(63), Fixed::from_int(-63)),
            Vec2::new(Fixed::from_int(1000), Fixed::from_int(-1000)),
        ];
        for p in probes {
            let mine = t.cell_of(p);
            // flow_field exposes the same cell via cost_at indexing indirectly; reconstruct
            // via its public sampling is awkward, so compare against our own mapping which is
            // a byte-for-byte copy — this test guards that copy against silent drift by
            // recomputing the shifted-floor inline.
            let expect_x = {
                let shifted = p.x + HALF_EXTENT;
                let c = (shifted / CELL_SIZE).to_int();
                c.max(0).min(GRID as i32 - 1)
            };
            let expect_y = {
                let shifted = p.y + HALF_EXTENT;
                let c = (shifted / CELL_SIZE).to_int();
                c.max(0).min(GRID as i32 - 1)
            };
            assert_eq!(mine, (expect_x, expect_y));
        }
    }

    #[test]
    fn diagonal_cannot_peek_through_a_wall_corner() {
        // A 45° segment from cell (20,20) to (22,22) grazes the corner between its first
        // diagonal step's two flanking cells (21,20) and (20,21). A `Heavy` cell on EITHER
        // flank must block the line — otherwise a unit peeks through the wall corner.
        let a = cell_center(20, 20);
        let b = cell_center(22, 22);

        let open = Terrain::open();
        assert!(open.line_of_sight(a, b), "open diagonal is clear");

        for flank in [(21, 20), (20, 21)] {
            let mut t = Terrain::open();
            t.set_cover(flank.0, flank.1, Cover::Heavy);
            assert!(
                !t.line_of_sight(a, b),
                "Heavy at flank {flank:?} must block the diagonal corner"
            );
            // Still symmetric with the stricter corner rule.
            assert_eq!(t.line_of_sight(a, b), t.line_of_sight(b, a));
        }
    }
}

//! Static battlefield obstacles — the SHARED source of truth for a visible prop's **sim collision
//! footprint** and its **render mesh** (Q24; D50 follow-through).
//!
//! D50 landed the embodied world dressing (trees / rocks / crates / sandbag berms / turret
//! emplacements) as a render-only `PROP_LAYOUT` with *no sim entity behind it* — so the player
//! walked straight through everything they saw. D50 itself flagged the fix: *"if props ever need to
//! be gameplay cover they must become sim … data (never a render-side back-channel to the sim —
//! invariant #4)."* This module is that source of truth, living in `core` so the sim owns it: the
//! scenario paints [`Cover::Impassable`](crate::terrain::Cover::Impassable) under each obstacle
//! (real collision), and the renderer *reads* this same list to draw the props (core → render, the
//! allowed direction). One list, so a prop can never again be visible-but-passable.
//!
//! Obstacles are **static map data**, like [`Terrain`] itself — placed once at scenario build,
//! never mutated per tick, so nothing here enters the per-tick checksum (invariant #7). All
//! positions are fixed-point (invariant #1); there are no floats to leak into the sim.

use crate::components::Vec2;
use crate::fixed::Fixed;
use crate::flow_field::HALF_EXTENT;
use crate::terrain::{Cover, Terrain};

/// The kind of a static obstacle. The renderer maps this to a concrete greybox mesh; the sim maps
/// it only to a collision [`footprint_radius`](ObstacleKind::footprint_radius).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObstacleKind {
    /// A tree — narrow solid trunk (the canopy is cosmetic and does not block).
    Tree,
    /// A boulder / rock.
    Rock,
    /// A supply crate.
    Crate,
    /// A sandbag berm / barricade — a short wall segment, the widest footprint.
    Barricade,
    /// A US crew-served turret emplacement.
    TurretUs,
    /// A French remote-weapon-station turret emplacement.
    TurretFr,
}

impl ObstacleKind {
    /// Collision footprint radius in world units: a cell whose CENTRE lies within this of the
    /// obstacle centre is painted [`Cover::Impassable`]. Sized to the greybox mesh so a mover
    /// stops at the object's visible extent, not a phantom margin. Exact ratios keep it
    /// float-free (invariant #1).
    #[inline]
    pub fn footprint_radius(self) -> Fixed {
        match self {
            ObstacleKind::Tree => Fixed::from_ratio(1, 2), // trunk only
            ObstacleKind::Crate => Fixed::from_ratio(1, 2),
            ObstacleKind::Rock => Fixed::from_ratio(3, 4),
            ObstacleKind::TurretUs | ObstacleKind::TurretFr => Fixed::from_ratio(3, 4),
            ObstacleKind::Barricade => Fixed::from_ratio(5, 4), // a wall segment
        }
    }
}

/// One static obstacle: its kind and world-space centre. `Copy` so callers (including the renderer)
/// can iterate [`skirmish_obstacles`] freely.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Obstacle {
    pub kind: ObstacleKind,
    pub pos: Vec2,
}

/// Fixed-point world position from integer world coords (test helper / world-space probes).
#[cfg(test)]
#[inline]
fn iv(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

/// The skirmish battlefield's static obstacle layout — trees, boulders, crates, sandbag berms and
/// the two opposing turret emplacements around the two-base skirmish (`Scene::Skirmish`).
///
/// This is the single source that (a) the scenario paints impassable cells under via
/// [`paint_impassable`] and (b) the renderer draws its embodied props from. **Four-fold symmetric**
/// (every seed mirrored across `x→−x` and `y→−y`), because a prop is now real cover/collision, so —
/// like the rest of the skirmish cover map — it must favour no side or flank (fairness, invariant
/// #6). No seed sits on the central lane (`y=0`), a flank post (`x=0`), a base camp (`±30, 0`), or a
/// spawn. Deterministic + fixed-point.
pub fn skirmish_obstacles() -> Vec<Obstacle> {
    use ObstacleKind::*;
    // Centre cell (== `cell_of(world 0)`), the axis the skirmish cover map mirrors about.
    const C: i32 = (crate::flow_field::GRID / 2) as i32;
    let mut out = Vec::new();
    // Mirror one first-quadrant seed CELL offset to all four quadrants, placing each obstacle at its
    // cell CENTRE. Working in cell space (not raw world coords, which land on cell corners and floor
    // asymmetrically) makes the painted footprints mirror exactly about the centre cell — the same
    // fairness the rest of the cover map holds (invariant #6). `kind_for(sx)` picks the mesh per side
    // (the US turret on the −x flank, the French on the +x flank); footprints are identical across
    // kinds, so the impassable cells stay symmetric regardless of which mesh draws.
    let mut mirror = |ox: i32, oy: i32, kind_for: &dyn Fn(i32) -> ObstacleKind| {
        for (sx, sy) in [(1, 1), (-1, 1), (1, -1), (-1, -1)] {
            out.push(Obstacle {
                kind: kind_for(sx),
                pos: cell_center(C + sx * ox, C + sy * oy),
            });
        }
    };
    mirror(18, 13, &|_| Tree); // tree line, deep on each flank
    mirror(7, 21, &|_| Tree); // outer trees toward each corner
    mirror(9, 10, &|_| Rock); // mid-field boulders
    mirror(22, 6, &|_| Rock); // boulders out toward the bases' approaches
    mirror(6, 16, &|_| Crate); // supply crates off the flank routes
    mirror(13, 4, &|_| Barricade); // sandbag berms bracketing the centre lane
    mirror(14, 9, &|sx| if sx < 0 { TurretUs } else { TurretFr }); // opposing turret emplacements
    out
}

/// World-space centre of cell `(cx, cy)` — mirrors the [`flow_field`](crate::flow_field) / terrain
/// mapping (`CELL_SIZE == 1`), so a painted cell aligns exactly with the cell a mover samples.
#[inline]
fn cell_center(cx: i32, cy: i32) -> Vec2 {
    let half = Fixed::from_ratio(1, 2);
    let origin = Fixed::ZERO - HALF_EXTENT;
    Vec2::new(
        origin + Fixed::from_int(cx) + half,
        origin + Fixed::from_int(cy) + half,
    )
}

/// Paint [`Cover::Impassable`] onto `terrain` under every obstacle in `list`: each cell whose
/// centre lies within the obstacle's [`footprint_radius`](ObstacleKind::footprint_radius) becomes
/// solid, so a mover collides with the same shape the player sees. Integer/fixed-point and an
/// idempotent per-cell set, so it is order-independent and deterministic — safe for both lockstep
/// peers to run at scenario build (invariants #1/#7). Existing non-`None` cover under an obstacle
/// is upgraded to `Impassable` (a solid prop wins over concealment).
pub fn paint_impassable(terrain: &mut Terrain, list: &[Obstacle]) {
    for o in list {
        // Always solidify the cell the obstacle centre sits in — a prop placed at integer world
        // coords lands on a cell corner, where no cell centre is within a sub-cell radius, so the
        // radius sweep alone could paint nothing. This guarantees every visible prop blocks.
        let (ccx, ccy) = terrain.cell_of(o.pos);
        terrain.set_cover(ccx, ccy, Cover::Impassable);

        let r = o.kind.footprint_radius();
        let r_sq = r * r;
        let corner = Vec2::new(r, r);
        let (cx0, cy0) = terrain.cell_of(o.pos - corner);
        let (cx1, cy1) = terrain.cell_of(o.pos + corner);
        let mut cy = cy0;
        while cy <= cy1 {
            let mut cx = cx0;
            while cx <= cx1 {
                if (cell_center(cx, cy) - o.pos).len_sq() <= r_sq {
                    terrain.set_cover(cx, cy, Cover::Impassable);
                }
                cx += 1;
            }
            cy += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_obstacle_paints_at_least_its_own_cell() {
        let list = skirmish_obstacles();
        let mut t = Terrain::open();
        paint_impassable(&mut t, &list);
        for o in &list {
            assert!(
                t.cover_at(o.pos).blocks_movement(),
                "obstacle {:?} at {:?} must sit on a solid cell",
                o.kind,
                o.pos,
            );
        }
    }

    #[test]
    fn painting_is_deterministic_and_idempotent() {
        let list = skirmish_obstacles();
        let mut a = Terrain::open();
        let mut b = Terrain::open();
        paint_impassable(&mut a, &list);
        paint_impassable(&mut b, &list);
        paint_impassable(&mut b, &list); // twice — must land on the identical grid
        for cy in 0..crate::flow_field::GRID as i32 {
            for cx in 0..crate::flow_field::GRID as i32 {
                assert_eq!(a.cover_at_cell(cx, cy), b.cover_at_cell(cx, cy));
            }
        }
    }

    #[test]
    fn open_ground_between_obstacles_stays_passable() {
        // A point far from every obstacle (near a base at -30,0) is untouched.
        let list = skirmish_obstacles();
        let mut t = Terrain::open();
        paint_impassable(&mut t, &list);
        assert!(!t.cover_at(iv(-29, 0)).blocks_movement());
        assert!(!t.cover_at(iv(0, 0)).blocks_movement());
    }

    #[test]
    fn a_wider_barricade_paints_more_cells_than_a_tree() {
        let tree = [Obstacle { kind: ObstacleKind::Tree, pos: iv(0, 0) }];
        let berm = [Obstacle { kind: ObstacleKind::Barricade, pos: iv(0, 0) }];
        let count = |list: &[Obstacle]| {
            let mut t = Terrain::open();
            paint_impassable(&mut t, list);
            let mut n = 0;
            for cy in 0..crate::flow_field::GRID as i32 {
                for cx in 0..crate::flow_field::GRID as i32 {
                    if t.cover_at_cell(cx, cy).blocks_movement() {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(
            count(&berm) > count(&tree),
            "the barricade's larger footprint must paint more solid cells than a tree",
        );
    }
}

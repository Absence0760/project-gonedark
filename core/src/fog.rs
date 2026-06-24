//! Fog of war (invariant #1 — fixed-point/integer; invariant #6 — going dark stays fair).
//!
//! Fog is a **pure, deterministic derivation** from sim state, NOT stored sim state: per the
//! netcode design (architecture.md §Netcode) every client holds the full world, so fog is a
//! client-side *presentation* filter. It is therefore computed on demand and deliberately
//! **not** folded into the per-tick checksum, and — critically — nothing here ever mutates
//! sim state (computing visibility must never desync lockstep).
//!
//! Two modes, mirroring the two layers (game-design §3, §6):
//! - [`command_visibility`] — full strategic vision: every cell a living unit of the faction
//!   can see (within its vision radius, with terrain line of sight).
//! - [`embodied_visibility`] — "the world goes dark" (invariant #6): vision collapses to ONLY
//!   what the single possessed avatar can see. This is the vision half of embodiment.
//!
//! IMPLEMENTATION OWNER: worker 4. Compiling stub. Fill in the bodies + inline tests; you own
//! the internal `Visibility` representation, but KEEP these three public signatures intact.

use crate::components::{EntityKind, Faction, Vec2};
use crate::ecs::{Entity, World};
use crate::fixed::Fixed;
use crate::flow_field::{CELL_SIZE, GRID, HALF_EXTENT};
use crate::terrain::Terrain;

/// Default per-unit sight radius (world units) when none is otherwise specified.
pub const DEFAULT_VISION: Fixed = Fixed::from_int(24);

/// A computed visibility mask over the playfield grid (worker 4 owns the representation;
/// a row-major `Vec<bool>` over `flow_field::GRID` cells is the expected shape).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Visibility {
    /// Row-major `GRID*GRID` visible flags. Empty == nothing visible.
    visible: Vec<bool>,
}

impl Visibility {
    /// Nothing visible (all cells dark).
    pub fn blank() -> Visibility {
        Visibility {
            visible: vec![false; crate::flow_field::GRID * crate::flow_field::GRID],
        }
    }

    /// Is the cell containing `pos` currently visible? Maps `pos` to its grid cell with the
    /// same clamped floor as `terrain`/`flow_field`, then reads the flag.
    pub fn is_visible(&self, pos: Vec2) -> bool {
        let (cx, cy) = world_to_cell(pos);
        self.visible[cell_index(cx, cy)]
    }

    /// Mark a cell visible. Out-of-bounds cells are ignored (the void beyond the playfield is
    /// never lit), so callers needn't pre-bounds-check a bounding-box sweep.
    #[inline]
    fn set_visible(&mut self, cx: i32, cy: i32) {
        if in_bounds(cx, cy) {
            let idx = cell_index(cx, cy);
            self.visible[idx] = true;
        }
    }
}

/// Reveal every cell within `vision` of `eye` that also has line of sight from `eye`.
///
/// Cost is bounded to the vision bounding box: only cells whose CENTER lies within `vision`
/// (squared compare, no sqrt) AND are unobstructed (`terrain.line_of_sight`) are lit. The
/// sweep is integer-cell ordered, so it is deterministic.
fn reveal_from(vis: &mut Visibility, terrain: &Terrain, eye: Vec2, vision: Fixed) {
    if vision <= Fixed::ZERO {
        return;
    }
    let vision_sq = vision * vision;

    // Bounding box of cells the vision disc can touch: eye ± vision, mapped to clamped cells.
    let (min_x, min_y) = world_to_cell(Vec2::new(eye.x - vision, eye.y - vision));
    let (max_x, max_y) = world_to_cell(Vec2::new(eye.x + vision, eye.y + vision));

    let mut cy = min_y;
    while cy <= max_y {
        let mut cx = min_x;
        while cx <= max_x {
            let center = cell_center(cx, cy);
            let d = center - eye;
            if d.len_sq() <= vision_sq && terrain.line_of_sight(eye, center) {
                vis.set_visible(cx, cy);
            }
            cx += 1;
        }
        cy += 1;
    }
}

/// Full command-layer vision for `faction`: the union of every living unit's sighted cells.
/// Buildings don't contribute (only `EntityKind::Unit`). Entities are swept in stable index
/// order, though the result (a union of bools) is order-independent.
pub fn command_visibility(world: &World, terrain: &Terrain, faction: Faction) -> Visibility {
    let mut vis = Visibility::blank();
    for i in 0..world.capacity() {
        if !world.is_index_alive(i) {
            continue;
        }
        if world.kind[i] != EntityKind::Unit {
            continue;
        }
        if world.faction[i] != faction {
            continue;
        }
        reveal_from(&mut vis, terrain, world.pos[i], world.vision[i]);
    }
    vis
}

/// Embodied vision (invariant #6): ONLY what the possessed `avatar` can see — the strategic
/// map goes dark. A dead avatar handle yields a blank mask (total darkness).
pub fn embodied_visibility(world: &World, terrain: &Terrain, avatar: Entity) -> Visibility {
    if !world.is_alive(avatar) {
        return Visibility::blank();
    }
    let mut vis = Visibility::blank();
    let i = avatar.index as usize;
    reveal_from(&mut vis, terrain, world.pos[i], world.vision[i]);
    vis
}

// --- Coordinate mapping (an EXACT mirror of terrain/flow_field's private mapping) -----------
//
// Kept byte-for-byte identical to `terrain.rs` so a unit's vision cell == its cover/LoS/pathing
// cell. If flow_field's mapping ever changes, this must change in lockstep.

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

/// World position at the CENTER of cell `(cx, cy)`. Cell `i` covers
/// `[-HALF_EXTENT + i, -HALF_EXTENT + i + 1)`, so its centre is at `+ i + 1/2`.
#[inline]
fn cell_center(cx: i32, cy: i32) -> Vec2 {
    let origin = Fixed::ZERO - HALF_EXTENT;
    Vec2::new(
        origin + Fixed::from_int(cx) + Fixed::HALF,
        origin + Fixed::from_int(cy) + Fixed::HALF,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{EntityKind, Faction, Vec2};
    use crate::ecs::{Entity, World};
    use crate::fixed::Fixed;
    use crate::terrain::Terrain;

    fn spawn_unit(world: &mut World, faction: Faction, pos: Vec2, vision: Fixed) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = pos;
        world.faction[i] = faction;
        world.kind[i] = EntityKind::Unit;
        world.vision[i] = vision;
        e
    }

    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    #[test]
    fn nearby_visible_far_dark() {
        // A unit at origin with vision 24. A point well inside is lit; one well beyond is dark.
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, at(0, 0), Fixed::from_int(24));
        let terrain = Terrain::open();

        let vis = command_visibility(&world, &terrain, Faction::Player);

        // Cell containing the origin and a near point are visible.
        assert!(vis.is_visible(at(0, 0)));
        assert!(vis.is_visible(at(10, 0)));
        assert!(vis.is_visible(at(0, 20)));
        // Beyond the 24-unit radius: dark.
        assert!(!vis.is_visible(at(40, 0)));
        assert!(!vis.is_visible(at(0, 40)));
    }

    #[test]
    fn vision_radius_boundary() {
        // Vision 10. A cell center at distance just under 10 is lit; one beyond 10 is not.
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, at(0, 0), Fixed::from_int(10));
        let terrain = Terrain::open();
        let vis = command_visibility(&world, &terrain, Faction::Player);

        // (8,0) cell center sits at x = 8 + HALF, within the radius of 10 → visible.
        assert!(vis.is_visible(at(8, 0)));
        // (15,0) cell center is past radius 10 → dark.
        assert!(!vis.is_visible(at(15, 0)));
    }

    #[test]
    fn faction_with_no_units_is_all_dark() {
        let mut world = World::new();
        // A Player unit exists, but we query the Enemy faction.
        spawn_unit(&mut world, Faction::Player, at(0, 0), Fixed::from_int(24));
        let terrain = Terrain::open();
        let vis = command_visibility(&world, &terrain, Faction::Enemy);

        assert_eq!(vis, Visibility::blank());
        assert!(!vis.is_visible(at(0, 0)));
    }

    #[test]
    fn buildings_do_not_grant_command_vision() {
        let mut world = World::new();
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = at(0, 0);
        world.faction[i] = Faction::Player;
        world.kind[i] = EntityKind::Building;
        world.vision[i] = Fixed::from_int(24);

        let terrain = Terrain::open();
        let vis = command_visibility(&world, &terrain, Faction::Player);
        assert_eq!(vis, Visibility::blank());
    }

    #[test]
    fn embodied_reveals_only_around_avatar() {
        // Two friendly units far apart. Embodying one must light ONLY its area; the other unit's
        // area stays dark (the strategic map goes dark — invariant #6).
        let mut world = World::new();
        let avatar = spawn_unit(&mut world, Faction::Player, at(-30, -30), Fixed::from_int(12));
        let _far = spawn_unit(&mut world, Faction::Player, at(30, 30), Fixed::from_int(12));
        let terrain = Terrain::open();

        let vis = embodied_visibility(&world, &terrain, avatar);

        // Around the avatar: lit.
        assert!(vis.is_visible(at(-30, -30)));
        assert!(vis.is_visible(at(-25, -30)));
        // The far friendly unit's area: dark.
        assert!(!vis.is_visible(at(30, 30)));
        // And the command view DOES light both (contrast), proving the difference is embodiment.
        let cmd = command_visibility(&world, &terrain, Faction::Player);
        assert!(cmd.is_visible(at(30, 30)));
    }

    #[test]
    fn dead_avatar_is_blank() {
        let mut world = World::new();
        let avatar = spawn_unit(&mut world, Faction::Player, at(0, 0), Fixed::from_int(24));
        world.despawn(avatar);
        let terrain = Terrain::open();

        let vis = embodied_visibility(&world, &terrain, avatar);
        assert_eq!(vis, Visibility::blank());
        assert!(!vis.is_visible(at(0, 0)));
    }

    #[test]
    fn heavy_wall_blocks_vision_line_of_sight() {
        // A unit at origin with generous vision; a heavy wall between it and a far cell blocks
        // that cell even though it is within radius.
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, at(0, 0), Fixed::from_int(24));
        let mut terrain = Terrain::open();
        // Wall a vertical column at cell x = 70 (world x ~6). Cell of world x: x+64 floored.
        // World x=6 → cell 70. Put a wall column there spanning the relevant rows.
        let wall_cx = terrain.cell_of(at(6, 0)).0;
        terrain.fill_rect(wall_cx, 0, wall_cx, GRID as i32 - 1, crate::terrain::Cover::Heavy);

        let vis = command_visibility(&world, &terrain, Faction::Player);
        // Near side (before the wall) is lit.
        assert!(vis.is_visible(at(3, 0)));
        // A cell beyond the wall, still inside the 24-radius, must be dark (LoS blocked).
        assert!(!vis.is_visible(at(12, 0)));
    }
}

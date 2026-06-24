//! Read-only render snapshot (invariant #4). The renderer interpolates between two of
//! these and converts the Q16.16 positions to float at *its* boundary — it never calls
//! back into the sim to mutate state. Carrying raw `Fixed` keeps `core` float-free.
//!
//! Phase 2 widens the snapshot so the presentation layer can *show* the new systems:
//! each unit carries its faction, health fraction, and whether it is a building; and the
//! snapshot lists the territory control points. None of this is sim state — it is a copy
//! taken for rendering, so it is not checksummed (invariant #7 covers the world itself).

use crate::components::{EntityKind, Faction, InputSource, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::territory::Territory;

/// One unit's renderable state at a tick.
#[derive(Clone, Debug)]
pub struct UnitSnapshot {
    /// The unit's world (ECS) index — the renderer matches command-layer selection against
    /// this to highlight selected units. Presentation only; not checksummed.
    pub entity_index: u32,
    pub pos: Vec2,
    pub vel: Vec2,
    pub embodied: bool,
    /// Which side it belongs to (drives the render color).
    pub faction: Faction,
    /// Health as a Fixed fraction in `[0, 1]` (the renderer draws a bar from this).
    pub health: Fixed,
    /// True for buildings (drawn larger / distinctly), false for units.
    pub building: bool,
}

/// One control point's renderable state at a tick.
#[derive(Clone, Debug)]
pub struct ControlPointSnapshot {
    pub pos: Vec2,
    pub owner: Faction,
    /// Capture progress toward the current contester, Fixed in `[0, 1]`.
    pub progress: Fixed,
}

/// An immutable copy of the renderable world at one sim tick.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub tick: u64,
    pub units: Vec<UnitSnapshot>,
    pub control_points: Vec<ControlPointSnapshot>,
}

impl Snapshot {
    pub fn capture(world: &World, territory: &Territory, tick: u64) -> Self {
        let mut units = Vec::new();
        for i in 0..world.capacity() {
            if !world.is_index_alive(i) {
                continue;
            }
            units.push(UnitSnapshot {
                entity_index: i as u32,
                pos: world.pos[i],
                vel: world.vel[i],
                embodied: world.input_source[i] == InputSource::Embodied,
                faction: world.faction[i],
                health: world.health[i].fraction(),
                building: world.kind[i] == EntityKind::Building,
            });
        }
        let control_points = territory
            .points
            .iter()
            .map(|p| ControlPointSnapshot {
                pos: p.pos,
                owner: p.owner,
                progress: p.progress,
            })
            .collect();
        Snapshot {
            tick,
            units,
            control_points,
        }
    }
}

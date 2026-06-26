//! Read-only render snapshot (invariant #4). The renderer interpolates between two of
//! these and converts the Q16.16 positions to float at *its* boundary — it never calls
//! back into the sim to mutate state. Carrying raw `Fixed` keeps `core` float-free.
//!
//! Phase 2 widens the snapshot so the presentation layer can *show* the new systems:
//! each unit carries its faction, health fraction, and whether it is a building; and the
//! snapshot lists the territory control points. None of this is sim state — it is a copy
//! taken for rendering, so it is not checksummed (invariant #7 covers the world itself).

use crate::components::{EntityKind, Faction, InputSource, UnitKind, Vec2};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::projectile::Projectile;
use crate::territory::Territory;
use crate::trig::Angle;

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
    /// The producible archetype — renderer maps Heavy→tank, Rifleman→infantry; presentation only,
    /// not checksummed.
    pub unit_kind: UnitKind,
    /// Chassis facing (binary-radian [`Angle`]) — the direction the hull/tracks point. The sim slews
    /// it toward the unit's velocity (`heading_system`) or the embodied stick (`drive_hull`); the
    /// renderer orients the body mesh by it (tank embodiment P7, D55). Presentation copy — the real
    /// sim state is checksummed at its source, this snapshot is not (invariant #4/#7).
    pub hull_heading: Angle,
    /// Gun bearing (binary-radian [`Angle`]) — for a tank, the turret yaws independently of the hull
    /// (`turret_speed > 0`); for turret-less units it tracks the hull and is cosmetically irrelevant
    /// (no separate turret mesh). Same absolute frame as `hull_heading` (`+X = 0`, CCW). The renderer
    /// yaws the tank's turret mesh by it (P7). Presentation copy, not checksummed.
    pub turret_yaw: Angle,
}

/// One in-flight shell's renderable state at a tick (tank embodiment P7, D55). A presentation copy
/// of a [`Projectile`] — enough to draw a tracer and extrapolate it smoothly between ticks. The
/// real projectile pool is the checksummed sim state (`Sim::fold`); this copy is not checksummed
/// (invariant #4/#7). All-`Fixed`, so it stays float-free in `core` (the renderer converts at its
/// boundary).
#[derive(Clone, Debug)]
pub struct ProjectileSnapshot {
    /// Ground-plane position (world units) this tick.
    pub pos: Vec2,
    /// Ground-plane velocity (world units/tick) — the renderer extrapolates `pos + vel·alpha` for a
    /// smooth tracer between sim ticks, and reads the travel direction (its yaw) from it.
    pub vel: Vec2,
    /// Height above the ground plane (world units) — the shell's vertical position (its arc).
    pub height: Fixed,
    /// Vertical velocity (world units/tick) — the renderer extrapolates `height + vz·alpha`.
    pub vz: Fixed,
    /// The firing side (drives the tracer tint).
    pub faction: Faction,
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
    /// In-flight shells to draw as tracers (tank embodiment P7). Embodied-only by construction
    /// (invariant #3 — only an embodied unit's `Fire` launches a ballistic shell), so every entry is
    /// a physical, transient object, not strategic map intel.
    pub projectiles: Vec<ProjectileSnapshot>,
}

impl Snapshot {
    pub fn capture(
        world: &World,
        territory: &Territory,
        projectiles: &[Projectile],
        tick: u64,
    ) -> Self {
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
                unit_kind: world.unit_kind[i],
                hull_heading: world.hull_heading[i],
                turret_yaw: world.turret_yaw[i],
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
        let projectiles = projectiles
            .iter()
            .map(|p| ProjectileSnapshot {
                pos: p.pos2d,
                vel: p.vel2d,
                height: p.height,
                vz: p.vz,
                faction: p.faction,
            })
            .collect();
        Snapshot {
            tick,
            units,
            control_points,
            projectiles,
        }
    }
}

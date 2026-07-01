//! Sim events — the deterministic, per-tick fact stream the systems emit and the
//! presentation layer (alerts, audio, HUD) consumes. Cleared at the top of every
//! [`Sim::step`](crate::sim::Sim::step) and refilled in stable system order, so the stream
//! is itself deterministic.
//!
//! Events are **derived, transient signal** — every field is a copy of state that is already
//! folded into the per-tick checksum (invariant #7), so the event vector is deliberately
//! *not* re-hashed into the checksum. It exists to surface "what just happened" to the
//! [`alerts`](crate::alerts) channel and the embodied audio mix (game-design §6) without the
//! presentation layer having to diff world snapshots.

use crate::components::{Faction, Vec2};
use crate::ecs::Entity;

/// One thing that happened this tick. Positions are carried so an alert can point a
/// direction (game-design §6: "alerts, not intel" — a directional flash + audio, no map
/// reveal) without the presentation layer reaching back into sim state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SimEvent {
    /// `entity` (of `faction`) took `amount` damage from `source`, at `pos`.
    Damaged {
        entity: Entity,
        faction: Faction,
        source: Entity,
        amount: crate::fixed::Fixed,
        pos: Vec2,
    },
    /// `entity` (of `faction`) was killed by `source`, at `pos`. The entity is despawned the
    /// same tick this fires.
    Killed {
        entity: Entity,
        faction: Faction,
        source: Entity,
        pos: Vec2,
    },
    /// A control point at `pos` flipped from `from` to `to` (territory capture).
    Captured {
        pos: Vec2,
        from: Faction,
        to: Faction,
    },
    /// `faction` finished producing a unit at `pos` (it now exists in the world).
    UnitProduced { faction: Faction, pos: Vec2 },
    /// `entity` (of `faction`) fired a shot this tick from `pos` — a *committed* trigger pull: a
    /// round was spent and the weapon began cooling, **whether or not it hit anything** (a downrange
    /// miss still fires here). Emitted only by the embodied fire path ([`combat::resolve_fire`](crate::combat::resolve_fire)),
    /// so it is first-person-only by construction (invariant #3). Presentation drives the muzzle
    /// flash / gun-crack / recoil off THIS — the authoritative rate of fire — instead of the held
    /// trigger, so a cooling-down or dry-clicked pull produces no flare.
    Fired {
        entity: Entity,
        faction: Faction,
        pos: Vec2,
    },
}

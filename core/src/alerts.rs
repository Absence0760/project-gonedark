//! The alert channel — the thin thread back to command while embodied (invariant #6,
//! game-design §6).
//!
//! While the player is embodied the strategic map goes dark; alerts are the *only* signal
//! back: **"alerts, not intel"** — a direction + kind ("taking fire on the east camp"), never
//! a map reveal. This module turns the deterministic [`SimEvent`] stream into directional
//! [`Alert`]s for one observing faction, which the host surfaces as a HUD flash + audio cue
//! (the embodied audio mix is the host's job; this is the data feed).
//!
//! Alerts are **presentation-derived** from events (which are themselves derived from
//! checksummed state), so the channel is NOT folded into the per-tick checksum and never
//! mutates sim state. It must stay *thin* by design — surfacing too much would undo the
//! blindness that is the whole game (this deliberately does NOT settle open-question Q1 on how
//! thin the thread is; it implements the current "alerts-only" lean as a mechanism).
//!
//! IMPLEMENTATION OWNER: worker 5. Compiling stub. Fill in `ingest` (+ any decay/dedup) and
//! inline tests; you own the internals, but KEEP the public signatures intact.

use crate::components::{EntityKind, Faction, Vec2};
use crate::ecs::World;
use crate::event::SimEvent;

/// What kind of thing an alert is telling the blind commander.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertKind {
    /// One of your units is under fire.
    TakingFire,
    /// You lost a unit.
    UnitLost,
    /// One of your buildings is under attack.
    BaseUnderAttack,
    /// You lost control of a territory point.
    TerritoryLost,
}

/// A single directional alert: what happened and roughly where (so the HUD can flash a
/// direction), stamped with the tick it fired on (for ordering / fade-out).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Alert {
    pub kind: AlertKind,
    pub pos: Vec2,
    pub tick: u64,
}

/// The rolling set of recent alerts for one observing faction.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct AlertChannel {
    pub recent: Vec<Alert>,
}

impl AlertChannel {
    pub fn new() -> Self {
        AlertChannel { recent: Vec::new() }
    }

    /// Fold this tick's `events` into alerts relevant to `faction` (its own units taking fire /
    /// dying, its buildings attacked, its points lost), stamping each with `tick`.
    ///
    /// "Alerts, not intel" (game-design §6): every alert is just a kind + a direction (`pos`),
    /// never a map reveal. `world` is read-only — used only to classify a damaged entity as a
    /// unit vs a building so a hit on a building raises [`AlertKind::BaseUnderAttack`]. Events
    /// for other factions, and `UnitProduced`, are ignored. Deterministic: events are folded
    /// in their (already-deterministic) stream order; nothing here touches sim state.
    pub fn ingest(&mut self, events: &[SimEvent], world: &World, faction: Faction, tick: u64) {
        for event in events {
            match *event {
                SimEvent::Damaged {
                    faction: f,
                    pos,
                    entity,
                    ..
                } if f == faction => {
                    // The entity may have been despawned the same tick — guard the index.
                    let idx = entity.index as usize;
                    let is_building =
                        idx < world.capacity() && world.kind[idx] == EntityKind::Building;
                    let kind = if is_building {
                        AlertKind::BaseUnderAttack
                    } else {
                        AlertKind::TakingFire
                    };
                    self.push(kind, pos, tick);
                }
                SimEvent::Killed {
                    faction: f, pos, ..
                } if f == faction => {
                    self.push(AlertKind::UnitLost, pos, tick);
                }
                SimEvent::Captured { from: f, pos, .. } if f == faction => {
                    self.push(AlertKind::TerritoryLost, pos, tick);
                }
                // Other factions' events and UnitProduced carry no alert for this observer.
                _ => {}
            }
        }
    }

    /// Append one alert. A tiny private seam so all pushes share a single shape (and a future
    /// cap/dedup has one place to live).
    fn push(&mut self, kind: AlertKind, pos: Vec2, tick: u64) {
        self.recent.push(Alert { kind, pos, tick });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{EntityKind, Faction, Vec2};
    use crate::ecs::{Entity, World};
    use crate::event::SimEvent;
    use crate::fixed::Fixed;

    fn pos(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    /// A world with one unit (index 0) and one building (index 1).
    fn world_unit_and_building() -> (World, Entity, Entity) {
        let mut w = World::new();
        let unit = w.spawn();
        let bldg = w.spawn();
        w.kind[bldg.index as usize] = EntityKind::Building;
        (w, unit, bldg)
    }

    #[test]
    fn damage_to_observer_unit_is_taking_fire() {
        let (w, unit, _) = world_unit_and_building();
        let mut ch = AlertChannel::new();
        let p = pos(3, 4);
        ch.ingest(
            &[SimEvent::Damaged {
                entity: unit,
                faction: Faction::Player,
                source: unit,
                amount: Fixed::from_int(5),
                pos: p,
            }],
            &w,
            Faction::Player,
            7,
        );
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].kind, AlertKind::TakingFire);
        assert_eq!(ch.recent[0].pos, p);
        assert_eq!(ch.recent[0].tick, 7);
    }

    #[test]
    fn damage_to_observer_building_is_base_under_attack() {
        let (w, _, bldg) = world_unit_and_building();
        let mut ch = AlertChannel::new();
        ch.ingest(
            &[SimEvent::Damaged {
                entity: bldg,
                faction: Faction::Player,
                source: bldg,
                amount: Fixed::from_int(5),
                pos: pos(1, 1),
            }],
            &w,
            Faction::Player,
            1,
        );
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].kind, AlertKind::BaseUnderAttack);
    }

    #[test]
    fn killed_is_unit_lost() {
        let (w, unit, _) = world_unit_and_building();
        let mut ch = AlertChannel::new();
        ch.ingest(
            &[SimEvent::Killed {
                entity: unit,
                faction: Faction::Player,
                source: unit,
                pos: pos(2, 2),
            }],
            &w,
            Faction::Player,
            3,
        );
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].kind, AlertKind::UnitLost);
    }

    #[test]
    fn captured_from_observer_is_territory_lost() {
        let (w, _, _) = world_unit_and_building();
        let mut ch = AlertChannel::new();
        ch.ingest(
            &[SimEvent::Captured {
                pos: pos(9, 9),
                from: Faction::Player,
                to: Faction::Enemy,
            }],
            &w,
            Faction::Player,
            5,
        );
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].kind, AlertKind::TerritoryLost);
    }

    #[test]
    fn events_for_other_factions_produce_nothing() {
        let (w, unit, bldg) = world_unit_and_building();
        let mut ch = AlertChannel::new();
        ch.ingest(
            &[
                SimEvent::Damaged {
                    entity: unit,
                    faction: Faction::Enemy,
                    source: unit,
                    amount: Fixed::from_int(5),
                    pos: pos(1, 1),
                },
                SimEvent::Killed {
                    entity: bldg,
                    faction: Faction::Enemy,
                    source: bldg,
                    pos: pos(1, 1),
                },
                SimEvent::Captured {
                    pos: pos(1, 1),
                    from: Faction::Enemy,
                    to: Faction::Player,
                },
                // UnitProduced is always ignored, even for the observer.
                SimEvent::UnitProduced {
                    faction: Faction::Player,
                    pos: pos(1, 1),
                },
            ],
            &w,
            Faction::Player,
            1,
        );
        assert!(ch.recent.is_empty());
    }

    #[test]
    fn despawned_entity_index_is_handled_without_panic() {
        let (w, _, _) = world_unit_and_building();
        let mut ch = AlertChannel::new();
        // An entity whose index is out of range (despawned/never-existed).
        let phantom = Entity {
            index: 999,
            generation: 0,
        };
        ch.ingest(
            &[SimEvent::Damaged {
                entity: phantom,
                faction: Faction::Player,
                source: phantom,
                amount: Fixed::from_int(1),
                pos: pos(0, 0),
            }],
            &w,
            Faction::Player,
            1,
        );
        // Out-of-range → classified as a unit (not a building) → TakingFire, no panic.
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].kind, AlertKind::TakingFire);
    }
}

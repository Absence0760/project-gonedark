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
use crate::ecs::{Entity, World};
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
    /// never a map reveal. `world` is read-only — used to classify a damaged entity as a unit
    /// vs a building so a hit on a building raises [`AlertKind::BaseUnderAttack`], and to resolve
    /// the shooter's position for the self-hit case (see `observed_avatar`). Events for other
    /// factions, and `UnitProduced`, are ignored. Deterministic: events are folded in their
    /// (already-deterministic) stream order; nothing here touches sim state.
    ///
    /// `observed_avatar` is the entity the observer is currently possessing (`None` while in the
    /// command view). It fixes a fairness bug (invariant #6): a `Damaged` event carries the
    /// *victim's* own position, which for every other unit tells the commander where on the map
    /// to worry — but when the **avatar itself** is hit, that position equals the avatar's own,
    /// so the bearing degenerates to a fixed "dead ahead" chevron that actively misleads (it
    /// looks directional but isn't). For that one case we point the alert at the *shooter*
    /// instead, so "you are under fire" honestly indicates the threat direction. This is
    /// presentation-derived data (the channel is never folded into the checksum), so reading the
    /// shooter's position here changes no sim state.
    pub fn ingest(
        &mut self,
        events: &[SimEvent],
        world: &World,
        faction: Faction,
        observed_avatar: Option<Entity>,
        tick: u64,
    ) {
        for event in events {
            match *event {
                SimEvent::Damaged {
                    faction: f,
                    pos,
                    entity,
                    source,
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
                    // Self-hit fairness fix: when the possessed avatar is the victim, its own
                    // position yields a zero bearing vector ("dead ahead"). Point at the shooter
                    // instead so the directional cue is honest. Fall back to the victim's
                    // position if the shooter's index is stale (despawned same tick).
                    let alert_pos = if observed_avatar == Some(entity) {
                        let sidx = source.index as usize;
                        if sidx < world.capacity() {
                            world.pos[sidx]
                        } else {
                            pos
                        }
                    } else {
                        pos
                    };
                    self.push(kind, alert_pos, tick);
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

    /// Drop alerts that have aged past the HUD's fade window, so `recent` stays bounded to the
    /// live window instead of growing for the whole match. The channel is scanned every embodied
    /// frame; without this it becomes a linearly-growing per-frame cost (and unbounded memory).
    ///
    /// `fade_ticks` is the render-side fade window (a `render::hud` constant); the caller supplies
    /// it so `core` need not depend on `render`. An alert stamped at tick `t` is kept while
    /// `tick < t + fade_ticks` — exactly the predicate the HUD uses to decide whether to draw it,
    /// so pruning here can never remove an alert the HUD would still show.
    pub fn prune(&mut self, tick: u64, fade_ticks: u64) {
        self.recent
            .retain(|a| tick < a.tick.saturating_add(fade_ticks));
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
            1,
        );
        // Out-of-range → classified as a unit (not a building) → TakingFire, no panic.
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].kind, AlertKind::TakingFire);
    }

    #[test]
    fn self_hit_points_at_the_shooter_not_dead_ahead() {
        // The observer is possessing `unit` (index 0). A shooter standing at a known offset
        // hits the avatar. The alert must point at the shooter's position, not the avatar's own
        // (which would collapse to a zero bearing → misleading "dead ahead" chevron).
        let mut w = World::new();
        let avatar = w.spawn();
        let shooter = w.spawn();
        let avatar_pos = pos(10, 10);
        let shooter_pos = pos(10, 3); // due "south" of the avatar
        w.pos[avatar.index as usize] = avatar_pos;
        w.pos[shooter.index as usize] = shooter_pos;

        let mut ch = AlertChannel::new();
        ch.ingest(
            &[SimEvent::Damaged {
                entity: avatar,
                faction: Faction::Player,
                source: shooter,
                amount: Fixed::from_int(5),
                pos: avatar_pos, // the event carries the VICTIM's position
            }],
            &w,
            Faction::Player,
            Some(avatar),
            42,
        );
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].kind, AlertKind::TakingFire);
        // Points at the shooter, giving a real (non-zero) bearing — not the avatar's own pos.
        assert_eq!(ch.recent[0].pos, shooter_pos);
        assert_ne!(ch.recent[0].pos, avatar_pos);
    }

    #[test]
    fn non_avatar_hits_still_report_the_victim_position() {
        // A different Player unit is hit while the observer is embodied in `avatar`. That alert
        // still uses the victim's own position (it tells the commander WHERE on the map to worry).
        let mut w = World::new();
        let avatar = w.spawn();
        let other = w.spawn();
        let shooter = w.spawn();
        let victim_pos = pos(4, 9);
        w.pos[shooter.index as usize] = pos(0, 0);

        let mut ch = AlertChannel::new();
        ch.ingest(
            &[SimEvent::Damaged {
                entity: other,
                faction: Faction::Player,
                source: shooter,
                amount: Fixed::from_int(2),
                pos: victim_pos,
            }],
            &w,
            Faction::Player,
            Some(avatar),
            1,
        );
        assert_eq!(ch.recent.len(), 1);
        assert_eq!(ch.recent[0].pos, victim_pos);
    }

    #[test]
    fn prune_drops_alerts_past_the_fade_window_and_bounds_growth() {
        let (w, unit, _) = world_unit_and_building();
        let mut ch = AlertChannel::new();
        let fade = 10u64;
        // Ingest one alert per tick over many ticks, pruning each tick as the host does.
        for tick in 0..1000u64 {
            ch.ingest(
                &[SimEvent::Damaged {
                    entity: unit,
                    faction: Faction::Player,
                    source: unit,
                    amount: Fixed::from_int(1),
                    pos: pos(1, 1),
                }],
                &w,
                Faction::Player,
                None,
                tick,
            );
            ch.prune(tick, fade);
            // Never grows past the live window — one alert per tick for at most `fade` ticks.
            assert!(ch.recent.len() as u64 <= fade);
        }
        // Everything now older than the window prunes to empty.
        ch.prune(2000, fade);
        assert!(ch.recent.is_empty());
    }
}

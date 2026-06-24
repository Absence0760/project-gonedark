//! Territory capture and control (invariant #1 — fixed-point, deterministic).
//!
//! The map carries a fixed set of [`ControlPoint`]s. Each tick `territory_system` counts the
//! living units of each faction within a point's capture radius; if exactly one faction
//! contests it, capture `progress` advances toward that faction and, past a threshold, the
//! point flips owner (emitting a [`SimEvent::Captured`]). Held points feed economy income via
//! [`Territory::controlled_count`]. A contested point (two factions present) stalls.
//!
//! Territory IS per-tick sim state and IS folded into the checksum (invariant #7), so its
//! field shape is pinned: `points: Vec<ControlPoint>`, each `{ pos, owner, progress }`.
//!
//! IMPLEMENTATION OWNER: worker 4. Compiling stub; fill in `territory_system` + inline tests.
//! KEEP the `Territory`/`ControlPoint` field shapes and the public signatures intact.

use crate::components::{EntityKind, Faction, Vec2};
use crate::ecs::World;
use crate::event::SimEvent;
use crate::fixed::Fixed;

/// Distance (world units) within which a unit counts toward capturing a point.
pub const CAPTURE_RADIUS: Fixed = Fixed::from_int(6);

/// Capture progress gained per tick by the sole contesting faction (0..=1 over `progress`).
pub const CAPTURE_RATE: Fixed = Fixed::from_ratio(1, 120);

/// One capturable point on the map. PINNED SHAPE (checksum folds these fields).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ControlPoint {
    /// Fixed world position of the point.
    pub pos: Vec2,
    /// Current owner (`Neutral` until first captured).
    pub owner: Faction,
    /// Capture progress toward the current contester, a Fixed in `[0, 1]`. At 1 the point
    /// flips to the contesting faction and progress resets.
    pub progress: Fixed,
}

impl ControlPoint {
    /// A neutral, uncaptured point at `pos`.
    pub const fn neutral(pos: Vec2) -> Self {
        ControlPoint {
            pos,
            owner: Faction::Neutral,
            progress: Fixed::ZERO,
        }
    }
}

/// The set of control points on the map.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Territory {
    pub points: Vec<ControlPoint>,
}

impl Territory {
    /// A map with no control points (the Phase 1 empty field).
    pub fn empty() -> Territory {
        Territory { points: Vec::new() }
    }

    /// How many points `faction` currently owns — the economy's income multiplier input.
    pub fn controlled_count(&self, faction: Faction) -> u32 {
        self.points.iter().filter(|p| p.owner == faction).count() as u32
    }
}

/// Advance one tick of territory capture over all control points.
///
/// For each point (in pinned index order, so the resulting `Territory` state — which is folded
/// into the checksum — is identical on every peer):
/// 1. Count alive **units** of `Player` and of `Enemy` within `CAPTURE_RADIUS` (squared
///    distance, no sqrt). `Neutral` units never capture; buildings never capture.
/// 2. Resolve the sole contester: exactly one side present → that faction; both present →
///    contested (stall, no change); neither present → no change.
/// 3. A sole contester equal to the owner secures the point (`progress` resets to 0). A
///    different sole contester advances `progress` by `CAPTURE_RATE`; at/above 1 the point
///    flips owner, emits [`SimEvent::Captured`], and `progress` resets to 0.
pub fn territory_system(world: &World, territory: &mut Territory, events: &mut Vec<SimEvent>) {
    let radius_sq = CAPTURE_RADIUS * CAPTURE_RADIUS;

    for point in territory.points.iter_mut() {
        let mut player_present = false;
        let mut enemy_present = false;

        // Iterate entities in stable index order (invariant #1 / #7).
        for i in 0..world.capacity() {
            if !world.is_index_alive(i) {
                continue;
            }
            if world.kind[i] != EntityKind::Unit {
                continue;
            }
            let faction = world.faction[i];
            // Only the two capturing factions matter; skip Neutral early.
            if faction != Faction::Player && faction != Faction::Enemy {
                continue;
            }
            let d = world.pos[i] - point.pos;
            if d.len_sq() <= radius_sq {
                match faction {
                    Faction::Player => player_present = true,
                    Faction::Enemy => enemy_present = true,
                    Faction::Neutral => {}
                }
            }
        }

        // Resolve the sole contester, if any.
        let contester = match (player_present, enemy_present) {
            (true, false) => Some(Faction::Player),
            (false, true) => Some(Faction::Enemy),
            // Contested (both) or empty (neither): leave owner/progress unchanged this tick.
            _ => None,
        };

        let Some(c) = contester else {
            continue;
        };

        if c == point.owner {
            // The owner holds it uncontested → secure; drop any partial capture.
            point.progress = Fixed::ZERO;
        } else {
            point.progress += CAPTURE_RATE;
            if point.progress >= Fixed::ONE {
                let old_owner = point.owner;
                events.push(SimEvent::Captured {
                    pos: point.pos,
                    from: old_owner,
                    to: c,
                });
                point.owner = c;
                point.progress = Fixed::ZERO;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{EntityKind, Faction, Vec2};
    use crate::ecs::{Entity, World};
    use crate::fixed::Fixed;

    /// Spawn a unit of `faction` at `pos` and return its handle.
    fn spawn_unit(world: &mut World, faction: Faction, pos: Vec2) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = pos;
        world.faction[i] = faction;
        world.kind[i] = EntityKind::Unit;
        e
    }

    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    /// Ticks of sole-contester presence guaranteed to flip a point. `CAPTURE_RATE` rounds down
    /// in Q16.16 (a tick adds 546 raw bits, not the exact ratio), so the flip lands a tick or
    /// two past 120; 200 is a safe upper bound that always crosses the threshold.
    const TICKS_TO_CAPTURE: usize = 200;

    #[test]
    fn lone_player_captures_neutral_point() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, at(0, 0));

        let mut territory = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let mut events = Vec::new();

        // Drive ticks until the point flips. Progress must rise monotonically and stay below 1
        // until the flip, and no Captured event fires before the flip.
        let mut flipped_at = None;
        let mut prev_progress = Fixed::ZERO;
        for tick in 0..TICKS_TO_CAPTURE {
            territory_system(&world, &mut territory, &mut events);
            if territory.points[0].owner == Faction::Player {
                flipped_at = Some(tick);
                break;
            }
            // Still Neutral: progress climbing, no event yet.
            assert_eq!(territory.points[0].owner, Faction::Neutral);
            assert!(territory.points[0].progress < Fixed::ONE);
            assert!(
                territory.points[0].progress > prev_progress,
                "progress climbs each tick while uncontested"
            );
            prev_progress = territory.points[0].progress;
            assert!(events.is_empty(), "no flip before threshold");
        }

        assert!(flipped_at.is_some(), "point should flip within the bound");
        assert_eq!(territory.points[0].owner, Faction::Player);
        assert_eq!(
            territory.points[0].progress,
            Fixed::ZERO,
            "progress resets on flip"
        );
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            SimEvent::Captured {
                pos: at(0, 0),
                from: Faction::Neutral,
                to: Faction::Player,
            }
        );
    }

    #[test]
    fn owner_holding_resets_partial_progress() {
        // A Player partly captures a Neutral point, then the point becomes Player-owned and the
        // same Player presence must SECURE it (progress drops to 0), never advance further.
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, at(0, 0));
        let mut territory = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let mut events = Vec::new();

        // Build some partial progress (still Neutral-owned).
        territory_system(&world, &mut territory, &mut events);
        territory_system(&world, &mut territory, &mut events);
        assert_eq!(territory.points[0].owner, Faction::Neutral);
        assert!(territory.points[0].progress > Fixed::ZERO);

        // Force ownership to Player and a stale partial progress, then tick: it must reset.
        territory.points[0].owner = Faction::Player;
        territory.points[0].progress = Fixed::HALF;
        territory_system(&world, &mut territory, &mut events);
        assert_eq!(territory.points[0].owner, Faction::Player);
        assert_eq!(territory.points[0].progress, Fixed::ZERO);
    }

    #[test]
    fn contested_point_does_not_change() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, at(0, 0));
        spawn_unit(&mut world, Faction::Enemy, at(0, 0));

        let mut territory = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let mut events = Vec::new();

        for _ in 0..TICKS_TO_CAPTURE * 2 {
            territory_system(&world, &mut territory, &mut events);
        }
        assert_eq!(territory.points[0].owner, Faction::Neutral);
        assert_eq!(territory.points[0].progress, Fixed::ZERO);
        assert!(events.is_empty(), "contested point emits no capture");
    }

    #[test]
    fn empty_point_unchanged() {
        let world = World::new(); // no entities at all
        let mut territory = Territory {
            points: vec![ControlPoint::neutral(at(3, 3))],
        };
        let mut events = Vec::new();

        for _ in 0..TICKS_TO_CAPTURE {
            territory_system(&world, &mut territory, &mut events);
        }
        assert_eq!(territory.points[0].owner, Faction::Neutral);
        assert_eq!(territory.points[0].progress, Fixed::ZERO);
        assert!(events.is_empty());
    }

    #[test]
    fn radius_boundary_inside_captures_outside_does_not() {
        // A point at origin. CAPTURE_RADIUS is 6 → radius_sq = 36.
        // A unit at (6,0) is exactly on the boundary: len_sq = 36 <= 36 → inside (captures).
        let mut inside_world = World::new();
        spawn_unit(&mut inside_world, Faction::Player, at(6, 0));
        let mut inside = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let mut ev_in = Vec::new();
        for _ in 0..TICKS_TO_CAPTURE {
            territory_system(&inside_world, &mut inside, &mut ev_in);
        }
        assert_eq!(
            inside.points[0].owner,
            Faction::Player,
            "unit on the radius boundary captures (<= comparison)"
        );

        // A unit at (7,0): len_sq = 49 > 36 → outside, never captures.
        let mut outside_world = World::new();
        spawn_unit(&mut outside_world, Faction::Player, at(7, 0));
        let mut outside = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let mut ev_out = Vec::new();
        for _ in 0..TICKS_TO_CAPTURE * 2 {
            territory_system(&outside_world, &mut outside, &mut ev_out);
        }
        assert_eq!(outside.points[0].owner, Faction::Neutral);
        assert_eq!(outside.points[0].progress, Fixed::ZERO);
        assert!(ev_out.is_empty());
    }

    #[test]
    fn neutral_unit_does_not_capture() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Neutral, at(0, 0));
        let mut territory = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let mut events = Vec::new();
        for _ in 0..TICKS_TO_CAPTURE {
            territory_system(&world, &mut territory, &mut events);
        }
        assert_eq!(territory.points[0].owner, Faction::Neutral);
        assert_eq!(territory.points[0].progress, Fixed::ZERO);
    }

    #[test]
    fn buildings_do_not_capture() {
        let mut world = World::new();
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = at(0, 0);
        world.faction[i] = Faction::Player;
        world.kind[i] = EntityKind::Building;

        let mut territory = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let mut events = Vec::new();
        for _ in 0..TICKS_TO_CAPTURE {
            territory_system(&world, &mut territory, &mut events);
        }
        assert_eq!(territory.points[0].owner, Faction::Neutral);
        assert_eq!(territory.points[0].progress, Fixed::ZERO);
    }

    #[test]
    fn controlled_count_counts_owners() {
        let territory = Territory {
            points: vec![
                ControlPoint {
                    pos: at(0, 0),
                    owner: Faction::Player,
                    progress: Fixed::ZERO,
                },
                ControlPoint {
                    pos: at(1, 0),
                    owner: Faction::Player,
                    progress: Fixed::ZERO,
                },
                ControlPoint {
                    pos: at(2, 0),
                    owner: Faction::Enemy,
                    progress: Fixed::ZERO,
                },
                ControlPoint::neutral(at(3, 0)),
            ],
        };
        assert_eq!(territory.controlled_count(Faction::Player), 2);
        assert_eq!(territory.controlled_count(Faction::Enemy), 1);
        assert_eq!(territory.controlled_count(Faction::Neutral), 1);
    }

    #[test]
    fn empty_territory_is_inert() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player, at(0, 0));
        let mut territory = Territory::empty();
        let mut events = Vec::new();
        territory_system(&world, &mut territory, &mut events);
        assert!(territory.points.is_empty());
        assert!(events.is_empty());
    }
}

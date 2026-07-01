//! The **troop-training** command-UI seam (roadmap Phase 2: "Troop-training UI — pick a unit
//! type, see cost + queue + ETA, set a rally point"). This is the command-view counterpart to
//! the `command_ui` module: it turns a unit-type choice made on a *selected camp* into
//! the [`Command::QueueProduction`] the deterministic sim already understands.
//!
//! It is pure presentation→intent mapping (the `command_ui` pattern): it
//! emits `Command`s, it never mutates sim state, and it has no GPU/camera dependency → it is
//! unit-testable. The cost/ETA/queue *display* is the renderer's job — see
//! `render::train_panel`; this module only emits the production intent and the rally seam.
//!
//! # Unit-type slot vocabulary
//!
//! `unit_slot` indexes a fixed on-screen list of producible unit archetypes, mirroring
//! [`UnitKind`]'s declaration order so the slot is a stable wire-free index:
//!
//! | slot | unit                    | sim command                                      |
//! |------|-------------------------|--------------------------------------------------|
//! | 0    | [`UnitKind::Rifleman`]  | `QueueProduction { camp, unit: Rifleman }`       |
//! | 1    | [`UnitKind::Heavy`]     | `QueueProduction { camp, unit: Heavy }`          |
//!
//! Any other slot value, `unit_slot == None`, or `camp == None` emits nothing — the player must
//! have both a selected camp and a chosen unit type for a production order to commit. The sim
//! itself still rejects an unaffordable / unbuilt / dead camp
//! ([`economy::queue_production`](gonedark_core::economy::queue_production)); this
//! seam does not pre-check those (it has no `&World`), it only forms the intent.
//!
//! # Rally point — the camp spawn-rally seam ([`Command::SetCampRally`])
//!
//! Setting a rally point for a camp's *produced* units emits
//! [`Command::SetCampRally { camp, rally }`](gonedark_core::sim::Command::SetCampRally): the sim
//! stores it on the producing building, and a freshly-produced unit inherits it as its FIRST order —
//! [`Order::MoveTo(rally)`](gonedark_core::components::Order::MoveTo) — so it walks off the pad toward
//! the rally instead of piling up [`Order::Idle`](gonedark_core::components::Order::Idle) on the camp
//! (`economy::economy_system`). That first move is a literal-executor order (invariant #3): the unit
//! just moves to the point, it makes no autonomous decision. (Distinct from the per-unit
//! [`Order::FallBack(rally)`](gonedark_core::components::Order::FallBack) retreat, which is a unit
//! order, not a building's spawn rally.)
//!
//! [`rally_commands`] is the presentation→intent mapping (the [`train_commands`] pattern): it takes
//! the selected camp + the tapped world point, quantizes the point to exact `Fixed` bits at the input
//! boundary via [`crate::world_to_fixed`] (invariant #1, so no float ever crosses into `core`) with
//! the pure [`rally_point`] step, and forms at most one `SetCampRally`. It never mutates sim state and
//! has no `&World`, so it does not pre-check the camp (the sim's `economy::set_camp_rally` no-ops a
//! dead / non-building handle).

use gonedark_core::components::{UnitKind, Vec2};
use gonedark_core::ecs::Entity;
use gonedark_core::sim::Command;

/// Map a `unit_slot` index to its [`UnitKind`], mirroring [`UnitKind`]'s declaration order. Returns
/// `None` for any slot outside the producible vocabulary (so an unknown slot emits nothing).
fn unit_for_slot(slot: u8) -> Option<UnitKind> {
    Some(match slot {
        0 => UnitKind::Rifleman,
        1 => UnitKind::Heavy,
        2 => UnitKind::Tank,
        3 => UnitKind::Medic,
        4 => UnitKind::AntiTank,
        _ => return None,
    })
}

/// Map a troop-training UI choice onto the sim production command for the selected camp.
///
/// Pure intent → `Command`s (the [`command_ui`](crate::command_ui) pattern): it never mutates sim
/// state and forms at most one [`Command::QueueProduction`].
///
/// - `unit_slot`: the unit-type button pressed this frame (slot 0 = Rifleman, slot 1 = Heavy; see
///   the module-level table). `None`, or any out-of-vocabulary slot, emits nothing.
/// - `camp`: the currently selected camp entity the unit is produced at. `None` emits nothing — a
///   production order has no meaning without a camp.
///
/// Emits a single-element `Vec` on a valid (slot, camp) pair, or an empty `Vec` otherwise. The sim
/// applies the cost / built-camp / affordability checks
/// ([`economy::queue_production`](gonedark_core::economy::queue_production)); this seam
/// only forms the intent (it has no `&World` to pre-check against).
pub fn train_commands(unit_slot: Option<u8>, camp: Option<Entity>) -> Vec<Command> {
    // Both a camp and an in-vocabulary unit slot are required, else nothing reaches the sim.
    let (Some(slot), Some(camp)) = (unit_slot, camp) else {
        return Vec::new();
    };
    let Some(unit) = unit_for_slot(slot) else {
        return Vec::new();
    };
    vec![Command::QueueProduction { camp, unit }]
}

/// The pure **rally-point quantization** step: turn the tapped world point into exact `Fixed` bits
/// at the input boundary via [`crate::world_to_fixed`] (invariant #1), so no float ever crosses into
/// `core`. Returns `None` when no rally point was tapped this frame. [`rally_commands`] wraps this to
/// form the sim command.
pub fn rally_point(target_world: Option<(f32, f32)>) -> Option<Vec2> {
    let (x, y) = target_world?;
    Some(Vec2::new(crate::world_to_fixed(x), crate::world_to_fixed(y)))
}

/// Map a troop-training rally-point choice onto the [`Command::SetCampRally`] for the selected camp.
///
/// Pure intent → `Command`s (the [`train_commands`] pattern): it never mutates sim state and forms at
/// most one `SetCampRally`. It quantizes the tapped point with [`rally_point`] (invariant #1).
///
/// - `target_world`: the world point tapped as the rally this frame. `None` (no tap) emits nothing.
/// - `camp`: the currently selected camp entity. `None` emits nothing — a rally has no meaning
///   without a camp to set it on.
///
/// Emits a single-element `Vec` on a valid (tap, camp) pair, or an empty `Vec` otherwise. The sim
/// applies the alive / is-a-building checks
/// ([`economy::set_camp_rally`](gonedark_core::economy::set_camp_rally)); this seam only forms the
/// intent (it has no `&World` to pre-check against).
pub fn rally_commands(target_world: Option<(f32, f32)>, camp: Option<Entity>) -> Vec<Command> {
    let (Some(rally), Some(camp)) = (rally_point(target_world), camp) else {
        return Vec::new();
    };
    vec![Command::SetCampRally { camp, rally }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camp_entity(index: u32) -> Entity {
        Entity {
            index,
            generation: 0,
        }
    }

    #[test]
    fn slot0_queues_a_rifleman_at_the_selected_camp() {
        let camp = camp_entity(3);
        let cmds = train_commands(Some(0), Some(camp));
        assert_eq!(cmds.len(), 1, "exactly one production command");
        match &cmds[0] {
            Command::QueueProduction { camp: c, unit } => {
                assert_eq!(*c, camp);
                assert_eq!(*unit, UnitKind::Rifleman);
            }
            other => panic!("expected QueueProduction(Rifleman), got {other:?}"),
        }
    }

    #[test]
    fn slot1_queues_a_heavy_at_the_selected_camp() {
        let camp = camp_entity(7);
        let cmds = train_commands(Some(1), Some(camp));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Command::QueueProduction { camp: c, unit } => {
                assert_eq!(*c, camp);
                assert_eq!(*unit, UnitKind::Heavy);
            }
            other => panic!("expected QueueProduction(Heavy), got {other:?}"),
        }
    }

    #[test]
    fn no_camp_emits_nothing() {
        assert!(
            train_commands(Some(0), None).is_empty(),
            "a unit choice without a selected camp must not produce"
        );
    }

    #[test]
    fn no_slot_emits_nothing() {
        let camp = camp_entity(1);
        assert!(
            train_commands(None, Some(camp)).is_empty(),
            "a selected camp without a unit choice must not produce"
        );
    }

    #[test]
    fn unknown_slot_emits_nothing() {
        let camp = camp_entity(1);
        for slot in [5u8, 6, 99, 255] {
            assert!(
                train_commands(Some(slot), Some(camp)).is_empty(),
                "slot {slot} is outside the producible vocabulary"
            );
        }
    }

    #[test]
    fn slots_cover_every_unit_kind_in_declaration_order() {
        // The slot vocabulary must map 1:1 onto UnitKind in declaration order, so the on-screen
        // index stays a stable, wire-free handle as kinds are added (D65 added Tank/Medic at 2/3;
        // D73 added AntiTank at 4).
        assert_eq!(unit_for_slot(0), Some(UnitKind::Rifleman));
        assert_eq!(unit_for_slot(1), Some(UnitKind::Heavy));
        assert_eq!(unit_for_slot(2), Some(UnitKind::Tank));
        assert_eq!(unit_for_slot(3), Some(UnitKind::Medic));
        assert_eq!(unit_for_slot(4), Some(UnitKind::AntiTank)); // D73
        assert_eq!(unit_for_slot(5), None);
    }

    // --- rally seam (quantization; no sim Command exists yet — flagged follow-up) ----------------

    #[test]
    fn rally_point_quantizes_the_tapped_world_point_to_fixed_bits() {
        let (x, y) = (12.5_f32, -4.25_f32);
        let rally = rally_point(Some((x, y))).expect("a tapped point yields a rally");
        // Bit-exact against the shared input-boundary quantizer (invariant #1).
        assert_eq!(rally.x.to_bits(), crate::world_to_fixed(x).to_bits());
        assert_eq!(rally.y.to_bits(), crate::world_to_fixed(y).to_bits());
    }

    #[test]
    fn rally_point_is_none_without_a_tap() {
        assert!(
            rally_point(None).is_none(),
            "no tapped point this frame → no rally"
        );
    }

    #[test]
    fn rally_commands_emits_set_camp_rally_for_a_tapped_point_at_the_selected_camp() {
        let camp = camp_entity(3);
        let (x, y) = (12.5_f32, -4.25_f32);
        let cmds = rally_commands(Some((x, y)), Some(camp));
        assert_eq!(cmds.len(), 1, "exactly one rally command");
        match &cmds[0] {
            Command::SetCampRally { camp: c, rally } => {
                assert_eq!(*c, camp);
                // Bit-exact against the shared input-boundary quantizer (invariant #1).
                assert_eq!(rally.x.to_bits(), crate::world_to_fixed(x).to_bits());
                assert_eq!(rally.y.to_bits(), crate::world_to_fixed(y).to_bits());
            }
            other => panic!("expected SetCampRally, got {other:?}"),
        }
    }

    #[test]
    fn rally_commands_needs_both_a_tap_and_a_camp() {
        let camp = camp_entity(1);
        assert!(
            rally_commands(None, Some(camp)).is_empty(),
            "a selected camp without a tap must not set a rally"
        );
        assert!(
            rally_commands(Some((1.0, 2.0)), None).is_empty(),
            "a tapped point without a selected camp must not set a rally"
        );
        assert!(
            rally_commands(None, None).is_empty(),
            "neither a tap nor a camp → nothing"
        );
    }
}

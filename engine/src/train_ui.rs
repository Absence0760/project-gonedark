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
//! # Rally point — a documented follow-up seam (NOT a new sim Command)
//!
//! Setting a rally point for a camp's *produced* units has **no sim `Command` today**. The only
//! rally concept in `core` is the per-unit [`Order::FallBack(rally)`](gonedark_core::components::Order::FallBack)
//! retreat order — that is a unit order, not a building's spawn rally, and produced units spawn
//! [`Order::Idle`](gonedark_core::components::Order::Idle) at the camp (`economy::economy_system`).
//! Per the task's hard boundary we do **not** invent a new sim `Command` here. Instead
//! [`rally_point`] exposes the pure input-boundary step — quantizing the tapped world point to
//! `Fixed` via [`crate::world_to_fixed`] (invariant #1) — and returns the `Vec2` for an integrator
//! to wire once a `Command::SetCampRally { camp, rally }` (or equivalent building-rally field +
//! "new units inherit the rally as their first Move") lands in `core::sim`. Until then this fn is
//! the tested quantization seam; emitting the order is the flagged follow-up.

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

/// The **rally-point** input-boundary seam for a camp's produced units.
///
/// There is **no sim `Command` for a camp rally today** (see the module docs): the only rally in
/// `core` is the per-unit `Order::FallBack` retreat, not a building's spawn rally. Per the task's
/// hard boundary this fn does **not** invent one — it performs only the pure, testable part the
/// integrator needs: quantizing the tapped world point to exact `Fixed` bits at the input boundary
/// via [`crate::world_to_fixed`] (invariant #1), so no float ever crosses into `core`.
///
/// Returns `None` when no rally point was tapped this frame. When a `Command::SetCampRally`
/// (or a building-rally field that new units inherit as their first `Move`) lands in `core::sim`,
/// the integrator wires this `Vec2` into it — **that command is the flagged follow-up**, not this
/// seam.
pub fn rally_point(target_world: Option<(f32, f32)>) -> Option<Vec2> {
    let (x, y) = target_world?;
    Some(Vec2::new(crate::world_to_fixed(x), crate::world_to_fixed(y)))
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
        for slot in [4u8, 5, 99, 255] {
            assert!(
                train_commands(Some(slot), Some(camp)).is_empty(),
                "slot {slot} is outside the producible vocabulary"
            );
        }
    }

    #[test]
    fn slots_cover_every_unit_kind_in_declaration_order() {
        // The slot vocabulary must map 1:1 onto UnitKind in declaration order, so the on-screen
        // index stays a stable, wire-free handle as kinds are added (D65 added Tank/Medic at 2/3).
        assert_eq!(unit_for_slot(0), Some(UnitKind::Rifleman));
        assert_eq!(unit_for_slot(1), Some(UnitKind::Heavy));
        assert_eq!(unit_for_slot(2), Some(UnitKind::Tank));
        assert_eq!(unit_for_slot(3), Some(UnitKind::Medic));
        assert_eq!(unit_for_slot(4), None);
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
}

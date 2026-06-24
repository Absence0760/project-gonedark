//! The order/stance command vocabulary on a small screen (roadmap Phase 2: "the real depth
//! layer", game-design §8). This is where "smart play" lives — NOT in the unit AI (invariant
//! #3: units are literal executors). This layer turns an on-screen vocabulary choice
//! ([`InputFrame::command_slot`] / [`InputFrame::long_press`]) plus the current [`Selection`]
//! plus the tapped world point into the `Command`s the deterministic sim already understands.
//!
//! It is pure presentation→intent mapping: it emits `Command`s, it never mutates sim state, and
//! the float→`Fixed` quantization for any world target goes through the engine's input-boundary
//! [`crate::world_to_fixed`] (invariant #1). No GPU/camera dependency → unit-testable.
//!
//! # Slot vocabulary
//!
//! `command_slot` indexes a fixed on-screen action vocabulary. Each action fans out across the
//! whole selection, one `Command` per unit, in selection order (deterministic):
//!
//! | slot | action            | sim command   | needs target? |
//! |------|-------------------|---------------|---------------|
//! | 0    | Move              | `Move`        | yes           |
//! | 1    | Attack-move       | `AttackMove`  | yes           |
//! | 2    | Stance: Hold fire | `SetStance(HoldFire)`   | no  |
//! | 3    | Stance: Return fire | `SetStance(ReturnFire)` | no  |
//! | 4    | Stance: Fire at will | `SetStance(FireAtWill)` | no  |
//!
//! Any other slot value, or `command_slot == None`, emits nothing. A target-requiring slot
//! (0/1) with no `target_world` also emits nothing — the on-screen vocabulary only acts when
//! the player has actually picked a destination. The stance slots (2/3/4) ignore the target.
//!
//! Patrol / hold-position / fall-back live in the sim's `Order`/`SetOrder` vocabulary but are
//! NOT yet wired here: exposing them is a separate change (the sim accepts only `Move`,
//! `AttackMove`, and `SetStance` per-unit through this seam today).
//!
//! `long_press` is a reserved gate (intended to open the context / radial menu); it carries no
//! behavior yet — the on-screen `command_slot` path is the whole vocabulary for now.

use crate::selection::Selection;
use gonedark_core::components::{Stance, Vec2};
use gonedark_core::sim::Command;

/// Map this frame's command-UI intent onto sim commands for the current selection.
///
/// - `command_slot`: the vocabulary button pressed this frame (see the module-level table:
///   0 = Move, 1 = Attack-move, 2/3/4 = stance Hold fire / Return fire / Fire at will).
/// - `long_press`: reserved "open context / confirm" gate — currently a no-op.
/// - `selection`: the units the action applies to (empty → emit nothing; the engine's legacy
///   single-avatar tap-to-move still handles the no-selection case).
/// - `target_world`: the world point tapped this frame, if any (required for Move/AttackMove).
///
/// Emits one `Command` per selected unit, in selection order, quantizing any world target via
/// [`crate::world_to_fixed`] (invariant #1).
pub fn commands_for(
    command_slot: Option<u8>,
    long_press: bool,
    selection: &Selection,
    target_world: Option<(f32, f32)>,
) -> Vec<Command> {
    // `long_press` is reserved for the context/radial menu; it gates nothing yet.
    let _ = long_press;

    // No selection → nothing (legacy single-avatar path handles the tap), and no slot → nothing.
    let slot = match command_slot {
        Some(s) if !selection.is_empty() => s,
        _ => return Vec::new(),
    };

    // Quantize the tapped world point to exact fixed-point bits at the input boundary.
    let target = target_world.map(|(x, y)| Vec2::new(crate::world_to_fixed(x), crate::world_to_fixed(y)));

    let mut out = Vec::with_capacity(selection.units.len());
    match slot {
        // 0 = Move, 1 = Attack-move: both require a destination; bail if none was tapped.
        0 | 1 => {
            let Some(target) = target else {
                return Vec::new();
            };
            for &entity in &selection.units {
                out.push(if slot == 0 {
                    Command::Move { entity, target }
                } else {
                    Command::AttackMove { entity, target }
                });
            }
        }
        // 2/3/4 = stance changes: no target needed.
        2..=4 => {
            let stance = match slot {
                2 => Stance::HoldFire,
                3 => Stance::ReturnFire,
                _ => Stance::FireAtWill,
            };
            for &entity in &selection.units {
                out.push(Command::SetStance { entity, stance });
            }
        }
        // Unknown slot → no vocabulary entry → nothing.
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::ecs::Entity;

    fn sel(units: &[(u32, u32)]) -> Selection {
        // `units` is public; other Selection fields (drag anchor) are private — start from
        // `new()` and populate the public selection list.
        let mut s = Selection::new();
        s.units = units
            .iter()
            .map(|&(index, generation)| Entity { index, generation })
            .collect();
        s
    }

    #[test]
    fn slot0_move_fans_out_per_unit_with_quantized_target() {
        let selection = sel(&[(1, 0), (2, 3)]);
        let (tx, ty) = (12.5_f32, -4.25_f32);
        let cmds = commands_for(Some(0), false, &selection, Some((tx, ty)));
        assert_eq!(cmds.len(), 2);
        let want_x = crate::world_to_fixed(tx).to_bits();
        let want_y = crate::world_to_fixed(ty).to_bits();
        for (cmd, &ent) in cmds.iter().zip(&selection.units) {
            match cmd {
                Command::Move { entity, target } => {
                    assert_eq!(*entity, ent);
                    assert_eq!(target.x.to_bits(), want_x);
                    assert_eq!(target.y.to_bits(), want_y);
                }
                other => panic!("expected Move, got {other:?}"),
            }
        }
    }

    #[test]
    fn slot1_attackmove_fans_out_per_unit_with_quantized_target() {
        let selection = sel(&[(4, 1), (5, 1)]);
        let (tx, ty) = (3.0_f32, 7.0_f32);
        let cmds = commands_for(Some(1), false, &selection, Some((tx, ty)));
        assert_eq!(cmds.len(), 2);
        let want_x = crate::world_to_fixed(tx).to_bits();
        let want_y = crate::world_to_fixed(ty).to_bits();
        for (cmd, &ent) in cmds.iter().zip(&selection.units) {
            match cmd {
                Command::AttackMove { entity, target } => {
                    assert_eq!(*entity, ent);
                    assert_eq!(target.x.to_bits(), want_x);
                    assert_eq!(target.y.to_bits(), want_y);
                }
                other => panic!("expected AttackMove, got {other:?}"),
            }
        }
    }

    #[test]
    fn stance_slots_emit_one_setstance_per_unit_without_target() {
        let selection = sel(&[(7, 0), (8, 0), (9, 2)]);
        for (slot, want) in [
            (2u8, Stance::HoldFire),
            (3, Stance::ReturnFire),
            (4, Stance::FireAtWill),
        ] {
            // No target supplied — stance changes must not require one.
            let cmds = commands_for(Some(slot), false, &selection, None);
            assert_eq!(cmds.len(), 3, "slot {slot} should fan out per unit");
            for (cmd, &ent) in cmds.iter().zip(&selection.units) {
                match cmd {
                    Command::SetStance { entity, stance } => {
                        assert_eq!(*entity, ent);
                        assert_eq!(*stance, want);
                    }
                    other => panic!("expected SetStance, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn move_or_attackmove_without_target_emits_nothing() {
        let selection = sel(&[(1, 0)]);
        assert!(commands_for(Some(0), false, &selection, None).is_empty());
        assert!(commands_for(Some(1), false, &selection, None).is_empty());
    }

    #[test]
    fn empty_selection_emits_nothing_for_any_slot() {
        let empty = sel(&[]);
        for slot in 0u8..=4 {
            assert!(
                commands_for(Some(slot), false, &empty, Some((1.0, 2.0))).is_empty(),
                "slot {slot} on empty selection should emit nothing"
            );
        }
    }

    #[test]
    fn none_slot_emits_nothing() {
        let selection = sel(&[(1, 0), (2, 0)]);
        assert!(commands_for(None, false, &selection, Some((1.0, 2.0))).is_empty());
    }

    #[test]
    fn unknown_slot_emits_nothing() {
        let selection = sel(&[(1, 0)]);
        assert!(commands_for(Some(99), true, &selection, Some((1.0, 2.0))).is_empty());
    }
}

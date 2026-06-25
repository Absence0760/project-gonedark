//! The order/stance command vocabulary on a small screen (roadmap Phase 2: "the real depth
//! layer", game-design §8). This is where "smart play" lives — NOT in the unit AI (invariant
//! #3: units are literal executors). This layer turns an on-screen vocabulary choice
//! ([`InputFrame::command_slot`] / [`InputFrame::long_press`]) plus the current selection plus
//! the tapped world point into the `Command`s the deterministic sim already understands.
//!
//! It is pure presentation→intent mapping: it emits `Command`s, it never mutates sim state, and
//! the float→`Fixed` quantization for any world coordinate goes through the engine's input-
//! boundary [`crate::world_to_fixed`] (invariant #1). No GPU/camera dependency → unit-testable.
//!
//! # Slot vocabulary
//!
//! `command_slot` indexes a fixed on-screen action vocabulary. Each action fans out across the
//! whole selection, one `Command` per unit, in selection order (deterministic):
//!
//! | slot | action               | sim command                          | needs target? |
//! |------|----------------------|--------------------------------------|---------------|
//! | 0    | Move                 | `Move`                               | yes           |
//! | 1    | Attack-move          | `AttackMove`                         | yes           |
//! | 2    | Stance: Hold fire    | `SetStance(HoldFire)`                | no            |
//! | 3    | Stance: Return fire  | `SetStance(ReturnFire)`              | no            |
//! | 4    | Stance: Fire at will | `SetStance(FireAtWill)`              | no            |
//! | 5    | Hold position        | `SetOrder(HoldPosition)`             | no            |
//! | 6    | Patrol               | `SetOrder(Patrol{a: unit, b: tap})`  | yes           |
//! | 7    | Fall back            | `SetOrder(FallBack(tap))`            | yes           |
//! | 8    | Arm retreat trigger  | `SetRetreatThreshold(30%)`           | no            |
//! | 9    | Disarm retreat       | `SetRetreatThreshold(0)`             | no            |
//!
//! Any other slot value, or `command_slot == None`, emits nothing. A target-requiring slot
//! (0/1/6/7) with no `target_world` also emits nothing — the on-screen vocabulary only acts when
//! the player has actually picked a destination. Patrol anchors leg `a` at each unit's CURRENT
//! position and leg `b` at the tapped point.
//!
//! `long_press` is a reserved gate (intended to open the context / radial menu); it carries no
//! behavior yet — the on-screen `command_slot` path is the whole vocabulary for now.

use gonedark_core::components::{Order, Stance, Vec2};
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::Command;

/// Health fraction the "arm retreat trigger" slot pre-programs a unit to fall back below. A
/// placeholder default (balance is untuned — [D24]) exposed as a one-tap mechanism; a finer
/// control (a slider) is a UX question this does not settle.
fn retreat_default() -> Fixed {
    Fixed::from_ratio(3, 10)
}

/// Map this frame's command-UI intent onto sim commands for the current selection.
///
/// - `command_slot`: the vocabulary button pressed this frame (see the module-level table).
/// - `long_press`: reserved "open context / confirm" gate — currently a no-op.
/// - `selected`: the units the action applies to, each with its CURRENT world position
///   (`(handle, (x, y))`). Empty → emit nothing (the engine's legacy single-avatar tap-to-move
///   handles the no-selection case). The position is used to anchor Patrol's first leg.
/// - `target_world`: the world point tapped this frame, if any (required for slots 0/1/6/7).
///
/// Emits one `Command` per selected unit, in selection order, quantizing every world coordinate
/// via [`crate::world_to_fixed`] (invariant #1).
pub fn commands_for(
    command_slot: Option<u8>,
    long_press: bool,
    selected: &[(Entity, (f32, f32))],
    target_world: Option<(f32, f32)>,
) -> Vec<Command> {
    // `long_press` is reserved for the context/radial menu; it gates nothing yet.
    let _ = long_press;

    // No selection → nothing (legacy single-avatar path handles the tap), and no slot → nothing.
    let slot = match command_slot {
        Some(s) if !selected.is_empty() => s,
        _ => return Vec::new(),
    };

    // Quantize the tapped world point to exact fixed-point bits at the input boundary.
    let target =
        target_world.map(|(x, y)| Vec2::new(crate::world_to_fixed(x), crate::world_to_fixed(y)));

    let mut out = Vec::with_capacity(selected.len());
    match slot {
        // 0 Move / 1 Attack-move / 6 Patrol / 7 Fall back all need a destination.
        0 | 1 | 6 | 7 => {
            let Some(target) = target else {
                return Vec::new();
            };
            for &(entity, (px, py)) in selected {
                out.push(match slot {
                    0 => Command::Move { entity, target },
                    1 => Command::AttackMove { entity, target },
                    // Patrol between the unit's current position (leg a) and the tap (leg b).
                    6 => Command::SetOrder {
                        entity,
                        order: Order::Patrol {
                            a: Vec2::new(crate::world_to_fixed(px), crate::world_to_fixed(py)),
                            b: target,
                            toward_b: true,
                        },
                    },
                    // 7: fall back to the tapped rally point.
                    _ => Command::SetOrder {
                        entity,
                        order: Order::FallBack(target),
                    },
                });
            }
        }
        // 2/3/4 stance changes (no target).
        2..=4 => {
            let stance = match slot {
                2 => Stance::HoldFire,
                3 => Stance::ReturnFire,
                _ => Stance::FireAtWill,
            };
            for &(entity, _) in selected {
                out.push(Command::SetStance { entity, stance });
            }
        }
        // 5 hold position (no target).
        5 => {
            for &(entity, _) in selected {
                out.push(Command::SetOrder {
                    entity,
                    order: Order::HoldPosition,
                });
            }
        }
        // 8 arm / 9 disarm the retreat trigger (no target).
        8 | 9 => {
            let fraction = if slot == 8 {
                retreat_default()
            } else {
                Fixed::ZERO
            };
            for &(entity, _) in selected {
                out.push(Command::SetRetreatThreshold { entity, fraction });
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

    /// Build a selection of `(entity, world_pos)` tuples from `(index, x, y)` triples. Generation
    /// is irrelevant to the mapping (it just round-trips the handle), so it is fixed at 0.
    fn sel(units: &[(u32, f32, f32)]) -> Vec<(Entity, (f32, f32))> {
        units
            .iter()
            .map(|&(index, x, y)| {
                (
                    Entity {
                        index,
                        generation: 0,
                    },
                    (x, y),
                )
            })
            .collect()
    }

    #[test]
    fn slot0_move_fans_out_per_unit_with_quantized_target() {
        let selection = sel(&[(1, 0.0, 0.0), (2, 0.0, 0.0)]);
        let (tx, ty) = (12.5_f32, -4.25_f32);
        let cmds = commands_for(Some(0), false, &selection, Some((tx, ty)));
        assert_eq!(cmds.len(), 2);
        let want_x = crate::world_to_fixed(tx).to_bits();
        let want_y = crate::world_to_fixed(ty).to_bits();
        for (cmd, &(ent, _)) in cmds.iter().zip(&selection) {
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
        let selection = sel(&[(4, 0.0, 0.0), (5, 0.0, 0.0)]);
        let (tx, ty) = (3.0_f32, 7.0_f32);
        let cmds = commands_for(Some(1), false, &selection, Some((tx, ty)));
        assert_eq!(cmds.len(), 2);
        let want_x = crate::world_to_fixed(tx).to_bits();
        let want_y = crate::world_to_fixed(ty).to_bits();
        for (cmd, &(ent, _)) in cmds.iter().zip(&selection) {
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
        let selection = sel(&[(7, 0.0, 0.0), (8, 0.0, 0.0), (9, 0.0, 0.0)]);
        for (slot, want) in [
            (2u8, Stance::HoldFire),
            (3, Stance::ReturnFire),
            (4, Stance::FireAtWill),
        ] {
            // No target supplied — stance changes must not require one.
            let cmds = commands_for(Some(slot), false, &selection, None);
            assert_eq!(cmds.len(), 3, "slot {slot} should fan out per unit");
            for (cmd, &(ent, _)) in cmds.iter().zip(&selection) {
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
    fn slot5_hold_position_emits_setorder_without_target() {
        let selection = sel(&[(1, 5.0, 6.0), (2, 7.0, 8.0)]);
        let cmds = commands_for(Some(5), false, &selection, None);
        assert_eq!(cmds.len(), 2);
        for (cmd, &(ent, _)) in cmds.iter().zip(&selection) {
            match cmd {
                Command::SetOrder { entity, order } => {
                    assert_eq!(*entity, ent);
                    assert_eq!(*order, Order::HoldPosition);
                }
                other => panic!("expected SetOrder(HoldPosition), got {other:?}"),
            }
        }
    }

    #[test]
    fn slot6_patrol_anchors_leg_a_at_unit_and_leg_b_at_tap() {
        // Each unit patrols between its OWN current position (a) and the shared tapped point (b).
        let selection = sel(&[(1, -3.0, 2.0), (2, 4.0, -1.0)]);
        let (tx, ty) = (10.0_f32, 10.0_f32);
        let cmds = commands_for(Some(6), false, &selection, Some((tx, ty)));
        assert_eq!(cmds.len(), 2);
        let want_b = (
            crate::world_to_fixed(tx).to_bits(),
            crate::world_to_fixed(ty).to_bits(),
        );
        for (cmd, &(ent, (px, py))) in cmds.iter().zip(&selection) {
            match cmd {
                Command::SetOrder {
                    entity,
                    order: Order::Patrol { a, b, toward_b },
                } => {
                    assert_eq!(*entity, ent);
                    assert!(*toward_b, "a fresh patrol heads toward b first");
                    assert_eq!(a.x.to_bits(), crate::world_to_fixed(px).to_bits());
                    assert_eq!(a.y.to_bits(), crate::world_to_fixed(py).to_bits());
                    assert_eq!((b.x.to_bits(), b.y.to_bits()), want_b);
                }
                other => panic!("expected SetOrder(Patrol), got {other:?}"),
            }
        }
    }

    #[test]
    fn slot7_fall_back_emits_setorder_to_tapped_rally() {
        let selection = sel(&[(1, 0.0, 0.0)]);
        let (tx, ty) = (-8.0_f32, 3.5_f32);
        let cmds = commands_for(Some(7), false, &selection, Some((tx, ty)));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Command::SetOrder {
                order: Order::FallBack(rally),
                ..
            } => {
                assert_eq!(rally.x.to_bits(), crate::world_to_fixed(tx).to_bits());
                assert_eq!(rally.y.to_bits(), crate::world_to_fixed(ty).to_bits());
            }
            other => panic!("expected SetOrder(FallBack), got {other:?}"),
        }
    }

    #[test]
    fn slot8_arms_retreat_trigger_slot9_disarms() {
        let selection = sel(&[(1, 0.0, 0.0), (2, 0.0, 0.0)]);

        let armed = commands_for(Some(8), false, &selection, None);
        assert_eq!(armed.len(), 2);
        for (cmd, &(ent, _)) in armed.iter().zip(&selection) {
            match cmd {
                Command::SetRetreatThreshold { entity, fraction } => {
                    assert_eq!(*entity, ent);
                    assert_eq!(fraction.to_bits(), retreat_default().to_bits());
                    assert!(*fraction > Fixed::ZERO);
                }
                other => panic!("expected SetRetreatThreshold, got {other:?}"),
            }
        }

        let disarmed = commands_for(Some(9), false, &selection, None);
        assert_eq!(disarmed.len(), 2);
        for cmd in &disarmed {
            match cmd {
                Command::SetRetreatThreshold { fraction, .. } => {
                    assert_eq!(*fraction, Fixed::ZERO)
                }
                other => panic!("expected SetRetreatThreshold(0), got {other:?}"),
            }
        }
    }

    #[test]
    fn target_requiring_slots_without_target_emit_nothing() {
        let selection = sel(&[(1, 1.0, 1.0)]);
        for slot in [0u8, 1, 6, 7] {
            assert!(
                commands_for(Some(slot), false, &selection, None).is_empty(),
                "slot {slot} needs a target and should emit nothing without one"
            );
        }
    }

    #[test]
    fn empty_selection_emits_nothing_for_any_slot() {
        let empty: Vec<(Entity, (f32, f32))> = Vec::new();
        for slot in 0u8..=9 {
            assert!(
                commands_for(Some(slot), false, &empty, Some((1.0, 2.0))).is_empty(),
                "slot {slot} on empty selection should emit nothing"
            );
        }
    }

    #[test]
    fn none_slot_emits_nothing() {
        let selection = sel(&[(1, 0.0, 0.0), (2, 0.0, 0.0)]);
        assert!(commands_for(None, false, &selection, Some((1.0, 2.0))).is_empty());
    }

    #[test]
    fn unknown_slot_emits_nothing() {
        let selection = sel(&[(1, 0.0, 0.0)]);
        assert!(commands_for(Some(99), true, &selection, Some((1.0, 2.0))).is_empty());
    }
}

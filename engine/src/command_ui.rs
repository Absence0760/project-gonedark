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
//! # Long-press: preview vs. commit
//!
//! `long_press` is the context/radial-menu gate. It is a pure *intent* signal — never an extra
//! command and never any unit autonomy (invariant #3: depth lives in the vocabulary, not the AI
//! brain). It models a two-edge interaction over the same vocabulary above:
//!
//! - **`long_press` alone** (no `command_slot`): a *Preview*. It opens the radial menu and reports
//!   the candidate action labels that apply to the current selection + target, but emits **no**
//!   `Command`s. Nothing reaches the sim until the player picks — so an order no longer commits on
//!   a single stray tap; the long-press previews, then a slot confirms.
//! - **`long_press` + a `command_slot`**: the *Commit* edge. This is the slot being chosen out of
//!   the open radial menu, and it emits **byte-identically** to the plain `command_slot` path
//!   ([`commands_for`]) — same fixed-point-quantized `Command`s, same per-unit selection order.
//!
//! [`radial_intent`] returns this distinction as a [`RadialIntent`]; [`commands_for`] keeps the
//! single-edge slot semantics (it ignores `long_press`) for the legacy direct-slot path and is the
//! exact emission [`radial_intent`]'s Commit edge defers to.

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
/// This is the single-edge, direct-slot path: it commits whatever `command_slot` names and
/// deliberately ignores `long_press` (the radial-menu preview/commit gating lives in
/// [`radial_intent`], whose Commit edge defers to exactly this function).
///
/// - `command_slot`: the vocabulary button pressed this frame (see the module-level table).
/// - `long_press`: ignored here — see [`radial_intent`] for the preview-vs-commit gate.
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
    // `long_press` does not gate this direct-slot path — the radial preview/commit distinction is
    // [`radial_intent`]'s job, and its Commit edge calls straight back here for the emission.
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

/// A long-press interaction over the command vocabulary, resolved to one of its two edges.
///
/// Pure intent (invariant #3: vocabulary depth, never unit autonomy). The `Preview` carries no
/// `Command`s — it is only the menu the player is about to choose from — and the `Commit` carries
/// the exact same bytes [`commands_for`] would for that slot.
///
/// Not `PartialEq`: the sim's `Command` is intentionally `Copy`-only (no `Eq`), so compare the
/// `Commit` payload via its fields / `to_bits()` rather than deriving equality here.
#[derive(Debug, Clone)]
pub enum RadialIntent {
    /// The radial menu is open (long-press, no slot chosen yet): these are the action labels that
    /// apply to the current selection + target. NO `Command`s are emitted — nothing reaches the
    /// sim until the player picks a slot.
    Preview(Vec<&'static str>),
    /// A slot was chosen out of the open menu (long-press + slot): commit, byte-identical to the
    /// direct-slot [`commands_for`] path.
    Commit(Vec<Command>),
    /// No long-press this frame → no radial interaction at all (the direct-slot path, if any, runs
    /// via [`commands_for`]).
    None,
}

/// Static label for a vocabulary slot (the radial-menu wedge text), or `None` for slots outside
/// the vocabulary. Matches the module-level slot table exactly.
fn slot_label(slot: u8) -> Option<&'static str> {
    Some(match slot {
        0 => "Move",
        1 => "Attack-move",
        2 => "Hold fire",
        3 => "Return fire",
        4 => "Fire at will",
        5 => "Hold position",
        6 => "Patrol",
        7 => "Fall back",
        8 => "Arm retreat",
        9 => "Disarm retreat",
        _ => return None,
    })
}

/// The candidate action labels the radial menu offers for the current selection + target, in slot
/// order. Empty selection → empty (nothing to act on). Target-requiring slots (0/1/6/7) are
/// dropped when no `target_world` is set, mirroring exactly which slots [`commands_for`] would
/// actually emit for this frame — the preview never advertises an action that would be a no-op.
fn menu_for(
    selected: &[(Entity, (f32, f32))],
    target_world: Option<(f32, f32)>,
) -> Vec<&'static str> {
    if selected.is_empty() {
        return Vec::new();
    }
    let has_target = target_world.is_some();
    (0u8..=9)
        .filter(|&slot| has_target || !matches!(slot, 0 | 1 | 6 | 7))
        .filter_map(slot_label)
        .collect()
}

/// Resolve a long-press interaction into a [`RadialIntent`] (preview vs. commit vs. none).
///
/// This is the radial-menu gate the plain-slot [`commands_for`] path deliberately leaves alone:
///
/// - `!long_press` → [`RadialIntent::None`] (no radial interaction this frame).
/// - `long_press` with **no** `command_slot` → [`RadialIntent::Preview`]: the applicable menu,
///   **no `Command`s emitted**.
/// - `long_press` **with** a `command_slot` → [`RadialIntent::Commit`]: emits identically to
///   `commands_for(command_slot, .., selected, target_world)` — same fixed-point-quantized
///   `Command`s, same per-unit selection order (invariant #1).
///
/// The arguments mirror [`commands_for`] so the Commit edge is a straight delegation.
pub fn radial_intent(
    command_slot: Option<u8>,
    long_press: bool,
    selected: &[(Entity, (f32, f32))],
    target_world: Option<(f32, f32)>,
) -> RadialIntent {
    if !long_press {
        return RadialIntent::None;
    }
    match command_slot {
        // Commit edge: a slot was picked out of the open menu → emit exactly the slot path. Pass
        // `long_press` through (it is ignored downstream) so this stays a literal delegation.
        Some(slot) => {
            RadialIntent::Commit(commands_for(Some(slot), long_press, selected, target_world))
        }
        // Preview edge: long-press alone opens the menu but commits nothing.
        None => RadialIntent::Preview(menu_for(selected, target_world)),
    }
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

    // --- long-press radial intent: preview vs. commit -------------------------------------------

    #[test]
    fn no_long_press_is_radial_none() {
        let selection = sel(&[(1, 0.0, 0.0)]);
        assert!(matches!(
            radial_intent(Some(0), false, &selection, Some((1.0, 2.0))),
            RadialIntent::None
        ));
        // Even with no slot, absence of long-press means no radial interaction at all.
        assert!(matches!(
            radial_intent(None, false, &selection, Some((1.0, 2.0))),
            RadialIntent::None
        ));
    }

    #[test]
    fn long_press_alone_previews_menu_and_emits_no_commands() {
        // long-press, no slot → Preview with the applicable labels, ZERO commands reach the sim.
        let selection = sel(&[(1, 5.0, 6.0), (2, 7.0, 8.0)]);
        let intent = radial_intent(None, true, &selection, Some((1.0, 2.0)));
        match intent {
            RadialIntent::Preview(menu) => {
                // With a target every vocabulary slot applies, in slot order.
                assert_eq!(
                    menu,
                    vec![
                        "Move",
                        "Attack-move",
                        "Hold fire",
                        "Return fire",
                        "Fire at will",
                        "Hold position",
                        "Patrol",
                        "Fall back",
                        "Arm retreat",
                        "Disarm retreat",
                    ]
                );
            }
            other => panic!("expected Preview, got {other:?}"),
        }
    }

    #[test]
    fn preview_without_target_hides_target_requiring_slots() {
        // No tapped point → the target-requiring slots (0/1/6/7) are not advertised, mirroring
        // exactly which slots commands_for would actually emit.
        let selection = sel(&[(1, 0.0, 0.0)]);
        match radial_intent(None, true, &selection, None) {
            RadialIntent::Preview(menu) => {
                assert_eq!(
                    menu,
                    vec![
                        "Hold fire",
                        "Return fire",
                        "Fire at will",
                        "Hold position",
                        "Arm retreat",
                        "Disarm retreat",
                    ]
                );
                for hidden in ["Move", "Attack-move", "Patrol", "Fall back"] {
                    assert!(!menu.contains(&hidden), "{hidden} needs a target; hide it");
                }
            }
            other => panic!("expected Preview, got {other:?}"),
        }
    }

    #[test]
    fn preview_on_empty_selection_is_empty_menu() {
        let empty: Vec<(Entity, (f32, f32))> = Vec::new();
        match radial_intent(None, true, &empty, Some((1.0, 2.0))) {
            RadialIntent::Preview(menu) => assert!(menu.is_empty()),
            other => panic!("expected empty Preview, got {other:?}"),
        }
    }

    /// Field-wise equality for two `Command`s (`Command` is `Copy`-only, no `Eq` — invariant #1
    /// keeps it minimal). Compares the discriminant + every payload via `to_bits()` so the byte-
    /// identity check below is exact.
    fn cmd_eq(a: &Command, b: &Command) -> bool {
        use Command::*;
        match (a, b) {
            (
                Move {
                    entity: e1,
                    target: t1,
                },
                Move {
                    entity: e2,
                    target: t2,
                },
            )
            | (
                AttackMove {
                    entity: e1,
                    target: t1,
                },
                AttackMove {
                    entity: e2,
                    target: t2,
                },
            ) => e1 == e2 && t1.x.to_bits() == t2.x.to_bits() && t1.y.to_bits() == t2.y.to_bits(),
            (
                SetStance {
                    entity: e1,
                    stance: s1,
                },
                SetStance {
                    entity: e2,
                    stance: s2,
                },
            ) => e1 == e2 && s1 == s2,
            (
                SetRetreatThreshold {
                    entity: e1,
                    fraction: f1,
                },
                SetRetreatThreshold {
                    entity: e2,
                    fraction: f2,
                },
            ) => e1 == e2 && f1.to_bits() == f2.to_bits(),
            (
                SetOrder {
                    entity: e1,
                    order: o1,
                },
                SetOrder {
                    entity: e2,
                    order: o2,
                },
            ) => e1 == e2 && order_eq(o1, o2),
            _ => false,
        }
    }

    /// Field-wise equality for the `Order` payloads `commands_for` emits (HoldPosition / Patrol /
    /// FallBack), comparing fixed-point coordinates by bits.
    fn order_eq(a: &Order, b: &Order) -> bool {
        match (a, b) {
            (Order::HoldPosition, Order::HoldPosition) => true,
            (Order::FallBack(p1), Order::FallBack(p2)) => {
                p1.x.to_bits() == p2.x.to_bits() && p1.y.to_bits() == p2.y.to_bits()
            }
            (
                Order::Patrol {
                    a: a1,
                    b: b1,
                    toward_b: t1,
                },
                Order::Patrol {
                    a: a2,
                    b: b2,
                    toward_b: t2,
                },
            ) => {
                t1 == t2
                    && a1.x.to_bits() == a2.x.to_bits()
                    && a1.y.to_bits() == a2.y.to_bits()
                    && b1.x.to_bits() == b2.x.to_bits()
                    && b1.y.to_bits() == b2.y.to_bits()
            }
            _ => false,
        }
    }

    #[test]
    fn long_press_plus_slot_commits_byte_identically_to_slot_path() {
        // The commit edge must equal the direct-slot path bit-for-bit for every vocabulary slot,
        // both with and without a target (invariant #1: same Commands, same selection order).
        let selection = sel(&[(1, -3.0, 2.0), (2, 4.0, -1.0)]);
        for &target in &[Some((12.5_f32, -4.25_f32)), None] {
            for slot in 0u8..=9 {
                let want = commands_for(Some(slot), false, &selection, target);
                match radial_intent(Some(slot), true, &selection, target) {
                    RadialIntent::Commit(got) => {
                        assert_eq!(
                            got.len(),
                            want.len(),
                            "commit edge for slot {slot} (target {target:?}) must match count"
                        );
                        for (g, w) in got.iter().zip(&want) {
                            assert!(
                                cmd_eq(g, w),
                                "commit edge for slot {slot} (target {target:?}) diverged: {g:?} vs {w:?}"
                            );
                        }
                    }
                    other => panic!("expected Commit for slot {slot}, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn long_press_plus_unknown_slot_commits_nothing() {
        // An out-of-vocabulary slot is still a Commit edge, but with no commands — identical to the
        // direct path's empty emission.
        let selection = sel(&[(1, 0.0, 0.0)]);
        match radial_intent(Some(99), true, &selection, Some((1.0, 2.0))) {
            RadialIntent::Commit(got) => assert!(got.is_empty()),
            other => panic!("expected empty Commit, got {other:?}"),
        }
    }
}

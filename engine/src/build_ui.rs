//! The **build** half of the command-view vocabulary: placing structures from the top-down view
//! (roadmap Phase 2 — "place/queue structures from the command view"). The sim already understands
//! `Command::Build { faction, kind, pos }` and `economy::build` applies it; this layer is the
//! missing on-screen seam that turns "player picked a structure + tapped a spot" into that command.
//!
//! Like [`command_ui`](crate::command_ui) it is pure presentation→intent mapping: it emits
//! `Command`s, it never mutates sim state, and the float→`Fixed` quantization for the placement
//! point goes through the engine's input-boundary [`crate::world_to_fixed`] (invariant #1). No
//! GPU/camera dependency → unit-testable without a window.
//!
//! # Slot vocabulary
//!
//! `building_slot` indexes a fixed on-screen build palette. It is written as a single slot→kind
//! lookup ([`slot_kind`]) so adding a building kind later is one match arm, nothing else. The table
//! MUST stay in lockstep with the render palette (`gonedark_render::build_menu`'s `PALETTE`), whose
//! entry index is this same slot.
//!
//! | slot | structure | sim command                                     |
//! |------|-----------|-------------------------------------------------|
//! | 0    | Camp      | `Build { faction, kind: Camp, pos: <tap> }`     |
//! | 1    | Barracks  | `Build { faction, kind: Barracks, pos: <tap> }` |
//!
//! Any other slot value, `building_slot == None`, or `placement_world == None` emits nothing — the
//! palette only acts once the player has chosen a structure *and* picked where it goes. (Unlike the
//! order vocabulary, a build always needs a placement point, so there is no no-target slot here.)
//! Affordability is **not** gated here: the sim's `economy::build` already refuses an unaffordable
//! placement (no debt), and the greying of unaffordable palette entries is a render concern
//! ([`gonedark_render::build_menu`]). Emitting the command unconditionally keeps this seam a pure
//! input map and lets the single authoritative spend check live in the deterministic sim.

use gonedark_core::components::{BuildingKind, Faction, Vec2};
use gonedark_core::sim::Command;

/// The structure a build-palette slot places, or `None` for a slot outside the palette. Matches the
/// module-level slot table exactly. Adding a building kind = one more arm here.
fn slot_kind(slot: u8) -> Option<BuildingKind> {
    Some(match slot {
        0 => BuildingKind::Camp,
        1 => BuildingKind::Barracks,
        _ => return None,
    })
}

/// Map a build-palette choice + a placement point onto a `Command::Build` for `faction`.
///
/// - `building_slot`: the palette button pressed this frame (see the module-level table). `None`,
///   or a slot outside the palette, emits nothing.
/// - `faction`: the side the structure is built for (the local player's faction in single-player;
///   carried so lockstep peers can build for any side).
/// - `placement_world`: the world point the player tapped/clicked to place the structure. `None`
///   emits nothing — a build always needs a spot.
///
/// Emits at most one `Command::Build`, quantizing the placement point to exact Q16.16 bits via
/// [`crate::world_to_fixed`] (invariant #1). Affordability is the sim's job (`economy::build`);
/// this seam never checks resources.
///
/// This is the tested, documented seam for the build-palette wiring (like [`command_ui`]'s
/// `commands_for`); the live command-view loop calls it through
/// [`crate::command_view_production_commands`], which feeds it the armed palette slot
/// ([`InputFrame::building_slot`](gonedark_pal::InputFrame::building_slot)) and the unprojected
/// cursor ground point.
pub fn build_commands(
    building_slot: Option<u8>,
    faction: Faction,
    placement_world: Option<(f32, f32)>,
) -> Vec<Command> {
    // Need both a chosen structure and a placement point, or nothing happens.
    let (Some(slot), Some((px, py))) = (building_slot, placement_world) else {
        return Vec::new();
    };
    let Some(kind) = slot_kind(slot) else {
        return Vec::new();
    };
    // Quantize the placement point to exact fixed-point bits at the input boundary.
    let pos = Vec2::new(crate::world_to_fixed(px), crate::world_to_fixed(py));
    vec![Command::Build { faction, kind, pos }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot0_places_a_camp_at_the_quantized_tap() {
        let (px, py) = (12.5_f32, -4.25_f32);
        let cmds = build_commands(Some(0), Faction::Player, Some((px, py)));
        assert_eq!(cmds.len(), 1, "one Build command for a chosen slot + point");
        let want_x = crate::world_to_fixed(px).to_bits();
        let want_y = crate::world_to_fixed(py).to_bits();
        match &cmds[0] {
            Command::Build { faction, kind, pos } => {
                assert_eq!(*faction, Faction::Player);
                assert_eq!(*kind, BuildingKind::Camp, "slot 0 = Camp");
                assert_eq!(pos.x.to_bits(), want_x, "x quantized via world_to_fixed");
                assert_eq!(pos.y.to_bits(), want_y, "y quantized via world_to_fixed");
            }
            other => panic!("expected Build, got {other:?}"),
        }
    }

    #[test]
    fn build_carries_the_requested_faction() {
        // The slot maps the same structure for whichever side is passed (lockstep peers build for
        // any faction); the command must carry it through unchanged.
        for faction in [Faction::Player, Faction::Enemy, Faction::Neutral] {
            let cmds = build_commands(Some(0), faction, Some((1.0, 2.0)));
            assert_eq!(cmds.len(), 1);
            match &cmds[0] {
                Command::Build { faction: f, .. } => assert_eq!(*f, faction),
                other => panic!("expected Build, got {other:?}"),
            }
        }
    }

    #[test]
    fn no_slot_emits_nothing() {
        assert!(
            build_commands(None, Faction::Player, Some((1.0, 2.0))).is_empty(),
            "no structure chosen → nothing"
        );
    }

    #[test]
    fn no_placement_point_emits_nothing() {
        assert!(
            build_commands(Some(0), Faction::Player, None).is_empty(),
            "no spot picked → nothing (a build always needs a placement)"
        );
    }

    #[test]
    fn unknown_slot_emits_nothing() {
        // A slot outside the palette is not a structure → no command, even with a valid point.
        for slot in [2u8, 3, 99, 255] {
            assert!(
                build_commands(Some(slot), Faction::Player, Some((3.0, 4.0))).is_empty(),
                "slot {slot} is outside the palette and should emit nothing"
            );
        }
    }

    #[test]
    fn slot_kind_table_matches_the_palette() {
        // Guard the slot→kind lookup directly so the documented table can't silently drift (D65
        // added the Barracks at slot 1).
        assert_eq!(slot_kind(0), Some(BuildingKind::Camp));
        assert_eq!(slot_kind(1), Some(BuildingKind::Barracks));
        assert_eq!(slot_kind(2), None, "no third structure yet");
    }
}

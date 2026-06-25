//! The **camp-upgrade** half of "command and grow your camps" (roadmap: upgrade trees — the
//! "growth" half), as a pure presentation→intent mapping — the [`command_ui`](crate::command_ui)
//! sibling for the order/stance vocabulary.
//!
//! Tapping the on-screen "Upgrade" button on the selected camp turns into the single
//! [`Command::Upgrade`] the deterministic sim already understands (`core::sim` applies it via
//! `core::economy::upgrade`, spending `economy::upgrade_cost(level)` and bumping the camp one tier).
//! This layer is pure intent: it emits a `Command`, it never mutates sim state, and it carries no
//! float across the boundary (the upgrade command has no world coordinate to quantize — invariant
//! #1 is satisfied trivially). No GPU/camera dependency → unit-testable.
//!
//! # Scope today vs. the prereq-tree follow-up
//!
//! The sim currently models **linear camp-tier leveling only**: one `BuildingKind::Camp`, a single
//! `level` counter, and `upgrade_cost(level) = 200 * (level + 1)`. There is no prerequisite-graph
//! machinery in `core` yet. So this intent fn is deliberately a one-button "level up the selected
//! camp" — the readable tier display lives render-side in `gonedark_render::upgrade_panel`.
//!
//! A **richer per-structure / per-unit prerequisite *tree*** (pick *which* upgrade out of several,
//! gated on prerequisites) is a **`core` follow-up**, not a presentation change: it needs new sim
//! state (an upgrade id/enum + a per-building owned-upgrade set), a new `Command` variant carrying
//! that id, and prerequisite + cost logic in `core::economy` — all checksum-folded sim work outside
//! this render/presentation seam. When that lands, this fn grows a `which: UpgradeId` argument and
//! emits the new command; the empty-case guards below stay the same shape.

use gonedark_core::ecs::Entity;
use gonedark_core::sim::Command;

/// Map an "upgrade the selected camp" UI intent onto sim commands.
///
/// Pure intent → `Command`s (mirrors [`command_ui::commands_for`](crate::command_ui::commands_for)):
/// emits exactly **one** [`Command::Upgrade`] when the player triggered the action *and* a camp is
/// selected, and **nothing** otherwise.
///
/// - `do_upgrade`: the button/edge fired this frame (e.g. an on-screen "Upgrade" tap). `false` →
///   emit nothing.
/// - `camp`: the currently-selected camp entity, if any. `None` → emit nothing (the button does
///   nothing without a camp to act on, exactly as the command vocabulary no-ops on empty selection).
///
/// The sim still authoritatively rejects the upgrade if the camp is unbuilt, the entity is stale,
/// or the faction can't afford `economy::upgrade_cost(level)` — this layer only expresses the
/// *intent*; affordability/legality is the sim's call (and the render-side panel previews it so the
/// player isn't surprised).
pub fn upgrade_commands(do_upgrade: bool, camp: Option<Entity>) -> Vec<Command> {
    match (do_upgrade, camp) {
        (true, Some(camp)) => vec![Command::Upgrade { camp }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(index: u32) -> Entity {
        Entity {
            index,
            generation: 0,
        }
    }

    #[test]
    fn trigger_with_selected_camp_emits_one_upgrade() {
        let camp = ent(7);
        let cmds = upgrade_commands(true, Some(camp));
        assert_eq!(cmds.len(), 1, "exactly one Upgrade command");
        match &cmds[0] {
            Command::Upgrade { camp: c } => assert_eq!(*c, camp, "targets the selected camp"),
            other => panic!("expected Upgrade, got {other:?}"),
        }
    }

    #[test]
    fn no_trigger_emits_nothing_even_with_a_camp() {
        assert!(
            upgrade_commands(false, Some(ent(3))).is_empty(),
            "the button must be pressed to upgrade"
        );
    }

    #[test]
    fn trigger_without_a_camp_emits_nothing() {
        assert!(
            upgrade_commands(true, None).is_empty(),
            "no selected camp → nothing to upgrade"
        );
    }

    #[test]
    fn no_trigger_no_camp_emits_nothing() {
        assert!(upgrade_commands(false, None).is_empty());
    }
}
